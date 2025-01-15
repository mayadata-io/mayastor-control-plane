---
title: Storage Tiering
authors:
  - "@croomes"
owners:

creation-date: 2022-07-04
last-updated: 2022-07-04
---

# Storage Tiering

## Table of Contents

- [Storage Tiering](#storage-tiering)
  - [Table of Contents](#table-of-contents)
  - [Summary](#summary)
  - [Problem](#problem)
  - [Current Solution](#current-solution)
  - [Proposal](#proposal)
    - [DiskPool Classification](#diskpool-classification)
    - [StorageClass Topology Constraints](#storageclass-topology-constraints)
    - [CSI Node Topology](#csi-node-topology)
    - [Volume Scheduling](#volume-scheduling)
  - [Test Plan](#test-plan)
  - [GA Criteria](#ga-criteria)

## Summary

This is a design proposal to support multiple classes of backend
storage within the same Mayastor cluster, aka Storage Tiering.

## Problem

Multiple storage types can be made available for Mayastor to consume: raw block
devices, logical volumes, cloud or network attached devices. Each can have their
own cost, performance, durability and consistency characteristics.

Cluster administrators want to provide their users a choice between different
storage tiers that they assemble from these storage types.

## Current Solution

Mayastor currently has no concept of tiering and treats all `DiskPools` equally.
When choosing where to provision a volume or replica, the `DiskPool` with the
greatest available capacity is chosen.

Topology has been implemented within the control plane for both nodes
and pools.

CSI Node Topology has been implemented, but it is currently only used to ensure
volumes are placed on nodes that run the Mayastor data plane.

For pools, no functionality has been exposed to users, but the scheduler
supports label-based placement constraints.

## Proposal

At a high level:

1. (done) Introduce a label-based classification mechanism on the `DiskPool`
   resource.
2. Pass a classification labels from the DiskPool CRI to the internal object.
3. Add topology constraints to the StorageClass.
4. Publish `DiskPool` classification topologies on Kubernetes `Node` and `CSINode`
   objects via a new controller.
5. (done) Add a classification-aware scheduling capability to the Mayastor control
   plane.

### DiskPool Classification

Classification data can be added to `DiskPools` using labels. The key prefix
should be pre-defined (e.g. `openebs.io/classification`). Keys names are defined
by the cluster administrator, and the value set to `true`. Example:
`openebs.io/classification/premium=true`.

This allows a node to expose multiple `DiskPools` while supporting Kubernetes
standard label selectors.

No changes are needed to the `DiskPool` CRD. This is the same mechanism used to
influence Pod scheduling in Kubernetes.

Example:

```yaml
apiVersion: openebs.io/v1alpha1
kind: DiskPool
metadata:
  labels:
    openebs.io/classification/premium: "true"
  name: diskpool-ksnk5
spec:
  disks:
  - /dev/sdc
  node: node-a
status:
  available: 34321989632
  capacity: 34321989632
  state: Online
  used: 0
```

Labels from the `DiskPool` CRI need to be passed to the internal `DiskPool`
object when calling the `put_node_pool` API endpint:

```go
    /// Create or import the pool, on failure try again. When we reach max error
    /// count we fail the whole thing.
    #[tracing::instrument(fields(name = ?self.name(), status = ?self.status) skip(self))]
    pub(crate) async fn create_or_import(self) -> Result<ReconcilerAction, Error> {
        ...

        labels.insert(
            String::from(utils::CREATED_BY_KEY),
            String::from(utils::DSP_OPERATOR),
        );

        // Add any classification labels to the DiskPool.
        for (k, v) in self.metadata.labels.as_ref().unwrap().iter() {
            match k.starts_with(utils::CLASSIFICATION_KEY_PREFIX) {
                true => {
                    labels.insert(k.clone(), v.clone());
                }
                false => {}
            }
        }
```

### StorageClass Topology Constraints

A StorageClass can be created for each tier. Topology constraints set in the
StorageClass will ensure that only nodes with DiskPools matching the constraint
will be selected for provisioning.

```yaml
kind: StorageClass
apiVersion: storage.k8s.io/v1
metadata:
  name: premium
parameters:
  repl: '1'
  protocol: 'nvmf'
  ioTimeout: '60'
provisioner: io.openebs.csi-mayastor
volumeBindingMode: WaitForFirstConsumer
allowedTopologies:
- matchLabelExpressions:
  - key: openebs.io/classification/premium
    values:
    - "true"
```

### CSI Node Topology

CSI drivers normally register the labels that they use for topology. They do
this by returning the label key and any currently supported values in the CSI
`NodeGetInfo` response. For example, they might return:

```protobuf
message NodeGetInfoResponse {
  node_id = "csi-node://nodeA";
  max_volumes_per_node = 16;
  accessible_topology = {"kubernetes.io/hostname": "nodeA", openebs.io/classification/premium: "true"}
}
```

The `NodeGetInfo` request is initiated by kubelet when the
`csi-driver-registrar` container starts and registers itself as a kubelet
plugin. It is only called once. It calls the `NodeGetInfo` endpoint on the
`csi-node` container.

Topology KV pairs returned in the `NodeGetInfoResponse` are added as labels on
the `Node` object:

```yaml
apiVersion: v1
kind: Node
metadata:
  annotations:
    csi.volume.kubernetes.io/nodeid: '{"disk.csi.azure.com":"aks-storagepool-25014071-vmss000000","io.openebs.csi-mayastor":"csi-node://aks-storagepool-25014071-vmss000000"}'
  labels:
    openebs.io/engine: mayastor
    openebs.io/classification/premium: "true"
  name: aks-storagepool-25014071-vmss000000
spec:
  ...
```

The topology keys are registered against the CSI driver on the `CSINode` object:

```yaml
apiVersion: storage.k8s.io/v1
kind: CSINode
metadata:
  name: aks-storagepool-25014071-vmss000000
spec:
  drivers:
  - name: io.openebs.csi-mayastor
    nodeID: csi-node://aks-storagepool-25014071-vmss000000
    topologyKeys:
    - kubernetes.io/hostname
    - openebs.io/classification/premium
```

There are two issues with using the CSI driver registration process to manage
tiering topology:

1. Since `NodeGetInfo` is only called once when `csi-driver-registrar` starts,
   any changes to node topology (e.g. adding a `DiskPool`) will not be reflected
   on the node labels. A mechanism to trigger plugin re-registration is needed.
   This could be achieved by adding a livenessProbe on `csi-driver-registrar`
   that queries an endpoint that fails if an update is required. This would
   cause the `csi-driver-registrar` to restart.
2. The `csi-node` process does not currently have knowledge of the `DiskPools`
   exposed on the node so it would need to request them from the control plane's
   REST endpoint so it can be returned in the `NodeGetInfo` response. This would
   require adding communication from the node to the API, which is not ideal.

Instead, a separate controller is proposed. It will run alongside the diskpool
controller will watch for `DiskPool` changes. If the `DiskPools` on a node have
classification labels and they do not match the `CSINode` topology keys and/or
`Node` labels, then the `CSINode`/`Node` objects will be updated.

There would be no need to alter the plugin registration process or add tiering
topology to the `NodeGetInfo` response as the controller would ensure the end
result is the same.

A feature flag could be used to enable/disable the controller.

### Volume Scheduling

No changes to volume scheduling are expected. Topology requirements are passed
via CSI `CreateVolume` requests and existing placement code is topology-aware.

## Test Plan

Topology-aware placement is already covered but may need extending.

## GA Criteria

TBC
