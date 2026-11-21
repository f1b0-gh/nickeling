let Nullable = (import "helpers.ncl").Nullable in
# Capabilities are operator declarations; no cluster discovery is performed.
let capabilities = {
  ingressClass | Nullable String | default = null,
  storageClass | Nullable String | default = null,
  networkPolicy | Bool | default = false,
} in
{
  kubernetes = fun available => capabilities & available & {
    distribution = "kubernetes",
    kubernetesVersion = "1.34.0",
    schemaVersion = "1.34.0",
  },
  k3s = fun available => capabilities & available & {
    distribution = "k3s",
    kubernetesVersion = "1.34.1",
    schemaVersion = "1.34.0",
    k3sVersion = "v1.34.1+k3s1",
  },
}
