let Nullable = (import "helpers.ncl").Nullable in
# A small composable HTTP workload. The final bundle contract validates all
# resources, including any extra raw resources the caller appends.
fun target input =>
let cfg = input | {
  name | String,
  namespace | String,
  image | String,
  port | std.number.Integer,
  replicas | std.number.Integer | default = 1,
  config | { _ | String } | default = {},
  secrets | Array String | default = [],
  ingressHost | Nullable String | default = null,
  tlsSecret | Nullable String | default = null,
  storage | Nullable {
    size | String,
    mountPath | String,
  } | default = null,
  network | {
    ingress | Array Dyn | default = [],
    egress | Array Dyn | default = [],
  } | default = {},
} in
let workloadLabels = { "app.kubernetes.io/name" = cfg.name } in
let resourceMetadata = { name = cfg.name, namespace = cfg.namespace, labels = workloadLabels } in
[
  {
    apiVersion = "apps/v1", kind = "Deployment", metadata = resourceMetadata,
    spec = {
      replicas = cfg.replicas,
      selector.matchLabels = workloadLabels,
      template = {
        metadata.labels = workloadLabels,
        spec = {
          automountServiceAccountToken = false,
          containers = [{
            name = cfg.name, image = cfg.image,
            ports = [{name = "http", containerPort = cfg.port}],
            envFrom = [{configMapRef.name = cfg.name}] @
              std.array.map (fun name => {secretRef.name = name}) cfg.secrets,
            volumeMounts = if cfg.storage == null then [] else
              [{name = "data", mountPath = cfg.storage.mountPath}],
          }],
          volumes = if cfg.storage == null then [] else
            [{name = "data", persistentVolumeClaim.claimName = cfg.name}],
        },
      },
    },
  },
  {
    apiVersion = "v1", kind = "Service", metadata = resourceMetadata,
    spec = {type = "ClusterIP", selector = workloadLabels,
      ports = [{name = "http", port = cfg.port, targetPort = "http"}]},
  },
  {apiVersion = "v1", kind = "ConfigMap", metadata = resourceMetadata, data = cfg.config},
  {
    apiVersion = "networking.k8s.io/v1", kind = "NetworkPolicy", metadata = resourceMetadata,
    spec = {podSelector.matchLabels = workloadLabels, policyTypes = ["Ingress", "Egress"],
      ingress = cfg.network.ingress, egress = cfg.network.egress},
  },
] @ (if cfg.ingressHost == null then [] else [{
  apiVersion = "networking.k8s.io/v1", kind = "Ingress", metadata = resourceMetadata,
  spec = {
    ingressClassName = target.ingressClass,
    rules = [{host = cfg.ingressHost, http.paths = [{path = "/", pathType = "Prefix",
      backend.service = {name = cfg.name, port.number = cfg.port}}]}],
    tls = if cfg.tlsSecret == null then [] else
      [{hosts = [cfg.ingressHost], secretName = cfg.tlsSecret}],
  },
}]) @ (if cfg.storage == null then [] else [{
  apiVersion = "v1", kind = "PersistentVolumeClaim", metadata = resourceMetadata,
  spec = {accessModes = ["ReadWriteOnce"], storageClassName = target.storageClass,
    resources.requests.storage = cfg.storage.size},
}])
