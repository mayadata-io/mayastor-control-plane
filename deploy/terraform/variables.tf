variable "registry" {
  type        = string
  description = "The docker registry to pull from"
  default     = "mayadata"
}

variable "tag" {
  type        = string
  description = "The default docker image tag to use when pulling images, this applies to mayadata images only"
  default     = "develop"
}

variable "etcd_image" {
  type        = string
  default     = "docker.io/bitnami/etcd:3.4.15-debian-10-r43"
  description = "etcd image to use"
}

variable "control_node" {
  type        = string
  default     = "ksnode-1"
  description = "The node on which control plane components are scheduled - soft requirement"
}

variable "control_resource_limits" {
  type = map(any)
  default = {
    "cpu"    = "1000m"
    "memory" = "1Gi"
  }
}
variable "control_resource_requests" {
  type = map(any)
  default = {
    "cpu"    = "250m"
    "memory" = "500Mi"
  }
}

variable "nats_image" {
  type        = string
  description = "nats image used by the nats deployment"
  default     = "nats:2.2.6-alpine3.13"
}

variable "msp_operator_image" {
  type        = string
  description = "msp operator image to use"
  default     = "mayastor-msp-operator"
}

variable "rest_image" {
  type    = string
  default = "mayastor-rest"
}

variable "core_image" {
  type    = string
  default = "mayastor-core"
}

variable "mayastor_image" {
  type        = string
  description = "mayastor image to use"
  default     = "mayastor"
}

variable "mayastor_hugepages_2Mi" {
  type        = string
  description = "amount of hugepages to allocate for mayastor"
  default     = "2Gi"
}

variable "mayastor_cpus" {
  type        = string
  description = "number of CPUs to use"
  default     = 2
}
variable "mayastor_cpu_list" {
  type        = string
  description = "List of cores to run on, eg: 2,3"
  default     = "2,3"
}

variable "mayastor_memory" {
  type        = string
  description = "amount of memory to request for mayastor"
  default     = "4Gi"
}

variable "mayastor_rust_log" {
  type        = string
  description = "The RUST_LOG environment filter for mayastor"
  default     = "debug"
}

variable "csi_agent_image" {
  type        = string
  description = "mayastor CSI agent image to use"
  default     = "mayastor-csi"
}

variable "csi_agent_grace_period" {
  type        = string
  description = "termination grace period in seconds for the mayastor CSI pod"
  default     = 30
}

variable "csi_registar_image" {
  type        = string
  default     = "k8s.gcr.io/sig-storage/csi-node-driver-registrar:v2.2.0"
  description = "CSI sidecars to use"
}

variable "csi_attacher_image" {
  type        = string
  default     = "quay.io/k8scsi/csi-attacher:v3.1.0"
  description = "csi-attacher to use"
}

variable "csi_provisioner" {
  type        = string
  default     = "quay.io/k8scsi/csi-provisioner:v2.1.1"
  description = "csi-provisioner to use"
}

variable "csi_controller_image" {
  type        = string
  description = "mayastor CSI controller image to use"
  default     = "mayastor-csi-controller"
}

variable "control_request_timeout" {
  type        = string
  description = "default request timeout for any NATS or GRPC request"
  default     = "5s"
}

variable "control_cache_period" {
  type        = string
  description = "the period at which a component updates its resource cache"
  default     = "30s"
}

variable "with_jaeger" {
  type        = bool
  description = "enables or disables the jaegertracing-operator"
  default     = true
}

variable "control_rust_log" {
  type        = string
  description = "The RUST_LOG environment filter for all control-plane components"
  default     = "info,core=debug,rest=debug,csi_controller=debug,mayastor_csi=debug,msp_operator=debug,common_lib=debug,grpc=debug"
}
