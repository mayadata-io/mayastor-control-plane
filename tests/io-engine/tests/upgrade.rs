use deployer_cluster::{Cluster, ClusterBuilder, FindVolumeRequest};
use openapi::{
    apis::volumes_api::tower::client::direct::Volumes, models, models::PublishVolumeBody,
};

use nvmeadm::parse_value;
use std::{collections::HashMap, convert::TryInto, path::PathBuf, str::FromStr, time::Duration};
use tokio::sync::oneshot::error::TryRecvError;

#[tokio::test]
#[ignore]
async fn upgrade() {
    let gb = 1024 * 1024 * 1024u64;
    let pool_size = 10 * gb;
    let volume_1_size = 8 * gb;
    let replicas = 3;
    let drain_nexus = true;

    let cluster = ClusterBuilder::builder()
        .with_io_engines(replicas)
        .with_csi(false, true)
        .with_tmpfs_pool(pool_size)
        .with_cache_period("1s")
        .with_reconcile_period(Duration::from_secs(2), Duration::from_secs(2))
        .with_agents(vec!["Core", "HaNode", "HaCluster"])
        .with_options(|o| o.with_isolated_io_engine(true).with_io_engine_cores(2))
        .build()
        .await
        .unwrap();

    let cli = cluster.rest_v00();
    let volumes_api = cli.volumes_api();
    let nodes_api = cli.nodes_api();

    cluster
        .composer()
        .exec(&cluster.csi_container(0), vec!["nvme", "disconnect-all"])
        .await
        .unwrap();

    let mut volume_1 = volumes_api
        .put_volume(
            &"ec4e66fd-3b33-4439-b504-d49aba53da26".try_into().unwrap(),
            models::CreateVolumeBody::new(
                models::VolumePolicy::new(true),
                replicas as u8,
                volume_1_size,
                false,
            ),
        )
        .await
        .unwrap();
    volume_1 = volumes_api
        .put_volume_target(
            &volume_1.spec.uuid,
            PublishVolumeBody::new_all(
                HashMap::new(),
                None,
                cluster.node(0).to_string(),
                models::VolumeShareProtocol::Nvmf,
                None,
                cluster.csi_node(0),
            ),
        )
        .await
        .unwrap();
    tracing::info!(
        "Volume replicas: {:?}",
        volume_1.state.target.as_ref().unwrap().children
    );
    let (_, r) = tokio::sync::oneshot::channel::<()>();
    let task = run_fio_vol(&cluster, volume_1.clone(), r).await;

    let mut subsys = NvmeSubsystem::lookup(&volume_1.spec.uuid.to_string()).unwrap();
    let mut path_number = subsys.paths.first().map(|p| p.instance).unwrap();

    tracing::info!("First controller is {path_number}");
    let mut drain_node = 99;
    for _ in 0..15 {
        drain_node = (drain_node + 1) % replicas;
        let drain_node = cluster.node(drain_node);
        let target_node = volume_1.spec.target.clone().unwrap().node;

        if !drain_nexus && target_node == drain_node.as_str() {
            continue;
        }

        tracing::info!("Draining node {drain_node}");
        nodes_api
            .put_node_drain(&drain_node, "drain")
            .await
            .unwrap();

        cluster.composer().restart(&drain_node).await.unwrap();

        'outer: loop {
            subsys.reload().unwrap();
            if let Some((p, false)) = subsys.path_n() {
                if target_node == drain_node.as_str() {
                    if p.state == "live" && p.instance != path_number {
                        path_number = p.instance;
                        tracing::info!("New controller is {path_number}");
                        break 'outer;
                    }
                } else if p.state == "live" {
                    break 'outer;
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;

        tracing::info!("Removing cordon from node {target_node}");
        nodes_api
            .delete_node_cordon(&drain_node, "drain")
            .await
            .unwrap();

        tracing::info!("Waiting volume Online...");
        volume_1 = wait_till_volume_online(volume_1, volumes_api)
            .await
            .unwrap();
        assert_eq!(
            volume_1.state.target.as_ref().unwrap().children.len(),
            replicas as usize
        );

        tracing::info!("Wait for 5 secs...");
        tokio::time::sleep(Duration::from_secs(5)).await;

        if task.is_finished() {
            tracing::error!("FINISHED");
            break;
        }
    }
    tracing::info!("Drain/Rebuild Loop Exit");

    let code = task.await.unwrap();
    cluster
        .composer()
        .exec(&cluster.csi_container(0), vec!["nvme", "disconnect-all"])
        .await
        .unwrap();

    assert_eq!(code, Some(0), "Fio Failure");

    volumes_api.del_volume(&volume_1.state.uuid).await.unwrap();
}

async fn run_fio_vol(
    cluster: &Cluster,
    volume: models::Volume,
    stop: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<Option<i64>> {
    run_fio_vol_verify(cluster, volume, stop, None, "300s").await
}
async fn run_fio_vol_verify(
    cluster: &Cluster,
    volume: models::Volume,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    verify: Option<bool>,
    time: &str,
) -> tokio::task::JoinHandle<Option<i64>> {
    let fio_builder = |device: &str| {
        let filename = format!("--filename={device}");
        let time = format!("--runtime={time}");
        vec![
            "fio",
            "--direct=1",
            "--ioengine=libaio",
            "--bs=4k",
            "--iodepth=16",
            "--verify=crc32",
            "--verify_fatal=1",
            "--numjobs=1",
            "--name=fio",
            "--random_generator=tausworthe64",
            "--loops=5",
            filename.as_str(),
        ]
        .into_iter()
        .chain(match verify {
            Some(true) => {
                vec![
                    "--readwrite=read",
                    "--verify_only",
                    "--verify_dump=1",
                    "--verify_state_load=1",
                ]
            }
            Some(false) => {
                vec![
                    "--readwrite=write",
                    "--verify_state_save=1",
                    "--time_based",
                    time.as_str(),
                ]
            }
            None => {
                vec!["--readwrite=randrw", "--verify_async=2", "--verify_dump=1"]
            }
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>()
    };

    let mut node = cluster.csi_node_client(0).await.unwrap();
    node.node_stage_volume(&volume, HashMap::new())
        .await
        .unwrap();

    let response = node
        .internal()
        .find_volume(FindVolumeRequest {
            volume_id: volume.spec.uuid.to_string(),
        })
        .await
        .unwrap();

    let device_path = response.into_inner().device_path;
    let device_path = device_path.trim_end();
    let fio_cmd = fio_builder(device_path);
    let fio_cmdline = fio_cmd
        .iter()
        .fold(String::new(), |acc, next| format!("{acc} {next}"));
    let composer = cluster.composer_nt().clone();
    let csi_node = cluster.csi_container(0);

    tokio::spawn(async move {
        let code = loop {
            let (code, out) = composer.exec(&csi_node, fio_cmd.clone()).await.unwrap();
            tracing::info!("{}: {}, code: {:?}", fio_cmdline, out, code);
            if code != Some(0) {
                return code;
            }
            assert_eq!(code, Some(0));

            if stop.try_recv().is_ok() || matches!(stop.try_recv(), Err(TryRecvError::Closed)) {
                break code;
            }
        };

        if code == Some(0) {
            node.node_unstage_volume(&volume).await.unwrap();
        }
        code
    })
}

async fn wait_till_volume_online(
    volume: models::Volume,
    volume_client: &dyn Volumes,
) -> Result<models::Volume, models::Volume> {
    wait_till_volume_online_tmo(volume, volume_client, Duration::from_secs(60 * 10)).await
}

async fn wait_till_volume_online_tmo(
    volume: models::Volume,
    volume_client: &dyn Volumes,
    timeout: Duration,
) -> Result<models::Volume, models::Volume> {
    let start = std::time::Instant::now();
    loop {
        let volume = volume_client.get_volume(&volume.spec.uuid).await.unwrap();

        if volume.state.status == models::VolumeStatus::Online {
            return Ok(volume);
        }

        if std::time::Instant::now() > (start + timeout) {
            tracing::error!(?volume, "Timeout waiting for the volume become online");
            return Err(volume);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[derive(Clone, Debug)]
struct NvmeSubsystem {
    /// The sysfs path.
    sysfs_path: PathBuf,
    /// NVMe Qualified Name (NQN).
    #[allow(dead_code)]
    nqn: String,
    /// One or more NVMe paths (ANA).
    paths: Vec<NvmePath>,
}
impl NvmeSubsystem {
    fn lookup(uuid: &str) -> Result<Self, ()> {
        use utils::constants::nvme_target_nqn_prefix;
        let expected_nqn = format!("{}:{uuid}", nvme_target_nqn_prefix());
        let paths = glob::glob("/sys/class/nvme-subsystem/nvme-subsys*").unwrap();
        for path in paths {
            let sysfs_path = path.unwrap();
            let nqn = parse_value::<String>(&sysfs_path, "subsysnqn").unwrap();
            if nqn != expected_nqn {
                continue;
            }
            let mut subsys = Self {
                sysfs_path,
                nqn,
                paths: vec![],
            };
            subsys.reload()?;
            return Ok(subsys);
        }
        Err(())
    }
    fn path_n(&self) -> Option<(NvmePath, bool)> {
        let path_number = self.paths.first().cloned();
        path_number.map(|n| (n, self.paths.len() > 1))
    }
    fn reload(&mut self) -> Result<(), ()> {
        self.paths.clear();
        let sysfs = self.sysfs_path.display().to_string();
        let pattern = format!("{sysfs}/nvme?");
        let paths = glob::glob(&pattern).unwrap();
        for path in paths {
            let path = path.unwrap();
            let name = path.strip_prefix(&sysfs).unwrap().display().to_string();
            let instance = u32::from_str(name.trim_start_matches("nvme")).unwrap();
            let nqn = parse_value::<String>(&path, "subsysnqn").unwrap();
            let state = parse_value::<String>(&path, "state").unwrap();
            let serial = parse_value::<String>(&path, "serial").unwrap();
            let model = parse_value::<String>(&path, "model").unwrap();

            if serial.is_empty() || model.is_empty() {
                debug_assert!(false, "SERIAL MODEL");
                continue;
            }
            self.paths.push(NvmePath {
                nqn,
                state,
                instance,
            })
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct NvmePath {
    #[allow(dead_code)]
    nqn: String,
    state: String,
    instance: u32,
}
