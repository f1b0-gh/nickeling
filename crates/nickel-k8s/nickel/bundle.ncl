let Nullable = (import "helpers.ncl").Nullable in
let contracts = {
  "apps/v1/Deployment" = import "contracts/upstream/v1.34.0/deployment-apps-v1.ncl",
  "apps/v1/StatefulSet" = import "contracts/upstream/v1.34.0/statefulset-apps-v1.ncl",
  "v1/Service" = import "contracts/upstream/v1.34.0/service-v1.ncl",
  "v1/ConfigMap" = import "contracts/upstream/v1.34.0/configmap-v1.ncl",
  "v1/Namespace" = import "contracts/upstream/v1.34.0/namespace-v1.ncl",
  "v1/PersistentVolumeClaim" = import "contracts/upstream/v1.34.0/persistentvolumeclaim-v1.ncl",
  "v1/ServiceAccount" = import "contracts/upstream/v1.34.0/serviceaccount-v1.ncl",
  "networking.k8s.io/v1/Ingress" = import "contracts/upstream/v1.34.0/ingress-networking-v1.ncl",
  "networking.k8s.io/v1/NetworkPolicy" = import "contracts/upstream/v1.34.0/networkpolicy-networking-v1.ncl",
} in
let Resource = std.contract.custom (fun label resource =>
  if !(std.is_record resource) then
    'Error {message = "A Kubernetes resource must be a record"}
  else if !(std.record.has_field "apiVersion" resource && std.record.has_field "kind" resource) then
    'Error {message = "apiVersion and kind are required"}
  else
    let key = "%{resource.apiVersion}/%{resource.kind}" in
    if std.record.has_field key contracts then
      std.contract.check (std.record.get key contracts) label resource
    else
      'Error {message = "Unsupported Kubernetes resource: %{key}"}
) in
{
  target | {
    distribution | String,
    kubernetesVersion | String,
    schemaVersion | String,
    k3sVersion | String | optional,
    ingressClass | Nullable String,
    storageClass | Nullable String,
    networkPolicy | Bool,
  },
  resources | Array Resource,
}
