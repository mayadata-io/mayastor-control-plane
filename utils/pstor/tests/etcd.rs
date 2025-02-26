use composer::{Binary, Builder, ComposeTest, ContainerSpec};
use pstor::{etcd::Etcd, Store, StoreKv, StoreKvWatcher, WatchEvent, WatchKey, WatchResult};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{SocketAddr, TcpStream},
    str::FromStr,
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};

static ETCD_ENDPOINT: &str = "0.0.0.0:2379";

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct TestStruct {
    name: String,
    value: u64,
    msg: String,
}

async fn recv_tmo<T>(tmo: Duration, mut rcv: mpsc::Receiver<T>) -> Result<T, String> {
    tokio::time::timeout(tmo, rcv.recv())
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no value".to_string())
}

#[tokio::test]
async fn etcd() {
    composer::initialize(
        std::path::Path::new(std::env!("WORKSPACE_ROOT"))
            .to_str()
            .unwrap(),
    );
    let test = Builder::new()
        .name("etcd")
        .add_container_spec(
            ContainerSpec::from_binary(
                "etcd",
                Binary::from_path("etcd").with_args(vec![
                    "--data-dir",
                    "/tmp/etcd-data",
                    "--advertise-client-urls",
                    "http://0.0.0.0:2379",
                    "--listen-client-urls",
                    "http://0.0.0.0:2379",
                ]),
            )
            .with_portmap("2379", "2379")
            .with_portmap("2380", "2380"),
        )
        .build()
        .await
        .unwrap();

    assert!(wait_for_etcd_ready(ETCD_ENDPOINT).is_ok(), "etcd not ready");

    tokio::time::timeout(Duration::from_secs(2), async {
        while !Etcd::new(ETCD_ENDPOINT).await.unwrap().online().await {}
    })
    .await
    .expect("etcd to be ready");

    let mut store = Etcd::new(ETCD_ENDPOINT).await.expect("to connect to etcd");

    let key = serde_json::json!("key");
    let mut data = TestStruct {
        name: "John Doe".to_string(),
        value: 100,
        msg: "Hello etcd".to_string(),
    };

    // Add an entry to the store, read it back and make sure it is correct.
    store
        .put_kv(&key.to_string(), &serde_json::json!(&data))
        .await
        .expect("Failed to 'put' to etcd");
    let v = store.get_kv(&key).await.expect("Failed to 'get' from etcd");
    let result: TestStruct = serde_json::from_value(v).expect("Failed to deserialise value");
    assert_eq!(data, result);

    // Start a watch which should send a message when the subsequent 'put'
    // event occurs.
    let (put_hdl, r) = spawn_watch(&key, &mut store).await;

    // Modify entry.
    data.value = 200;
    store
        .put_kv(&key.to_string(), &serde_json::json!(&data))
        .await
        .expect("Failed to 'put' to etcd");

    // Wait up to 1 second for the watch to see the put event.
    let msg = recv_tmo(Duration::from_secs(1), r)
        .await
        .expect("Timed out waiting for message");
    let result: TestStruct = match msg {
        WatchEvent::Put(_k, v) => serde_json::from_value(v).expect("Failed to deserialise value"),
        _ => panic!("Expected a 'put' event"),
    };
    assert_eq!(result, data);

    // Start a watch which should send a message when the subsequent 'delete'
    // event occurs.
    let (del_hdl, r) = spawn_watch(&key, &mut store).await;
    store.delete_kv(&key).await.unwrap();

    // Wait up to 1 second for the watch to see the delete event.
    let msg = recv_tmo(Duration::from_secs(1), r)
        .await
        .expect("Timed out waiting for message");
    match msg {
        WatchEvent::Delete => {
            // The entry is deleted. Let's check that a subsequent 'get' fails.
            store
                .get_kv(&key)
                .await
                .expect_err("Entry should have been deleted");
        }
        _ => panic!("Expected a 'delete' event"),
    };

    put_hdl.await.unwrap();
    del_hdl.await.unwrap();

    test_kv_watcher(test).await.unwrap()
}

async fn test_kv_watcher(_test: ComposeTest) -> Result<(), etcd_client::Error> {
    utils::tracing_telemetry::TracingTelemetry::builder().init("etcd");

    let mut store = Etcd::new(ETCD_ENDPOINT).await.expect("to connect to etcd");

    let counter = std::sync::Arc::new(std::sync::Mutex::new(0));
    let updates = 10;
    let (s, r) = mpsc::channel(1);

    let watcher = store.kv_watcher(move |_arg| {
        *counter.lock().unwrap() += 1;
        if *counter.lock().unwrap() == updates {
            s.try_send(()).ok();
        }
        WatchResult::Continue
    });

    let volumes = std::iter::repeat_n(0, updates)
        .enumerate()
        .map(|(i, _)| format!("/volume/{i}/nexus"))
        .collect::<Vec<_>>();

    for volume in &volumes {
        watcher
            .watch(
                WatchKey::new(volume).with_rev(Some(1)),
                uuid::Uuid::new_v4(),
            )
            .unwrap();
    }

    for volume in &volumes {
        store
            .put_kv(&format!("{}/info", volume), &"b")
            .await
            .unwrap();
    }
    recv_tmo(Duration::from_secs(1), r)
        .await
        .expect("Should receive all updates timely");

    Ok(())
}

/// Spawn a watch thread which watches for a single change to the entry with
/// the given key.
async fn spawn_watch<W: Store>(
    key: &serde_json::Value,
    store: &mut W,
) -> (JoinHandle<()>, mpsc::Receiver<WatchEvent>) {
    let (s, r) = mpsc::channel(1);
    let mut watch = store.watch_kv(&key).await.expect("Failed to watch");
    let hdl = tokio::spawn(async move {
        match watch.recv().await.unwrap() {
            Ok(event) => {
                s.send(event).await.unwrap();
            }
            Err(_) => {
                panic!("Failed to receive event");
            }
        }
    });
    (hdl, r)
}

/// Wait to establish a connection to etcd.
/// Returns 'Ok' if connected otherwise 'Err' is returned.
fn wait_for_etcd_ready(endpoint: &str) -> io::Result<TcpStream> {
    let sa = SocketAddr::from_str(endpoint).unwrap();
    TcpStream::connect_timeout(&sa, Duration::from_secs(3))
}
