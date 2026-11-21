let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let application = import "/libraries/nickel-k8s/application.ncl" in
let chosenTarget = profiles.k3s {
  ingressClass = "traefik",
  storageClass = "local-path",
  networkPolicy = true,
} in
{
  target = chosenTarget,
  resources = application chosenTarget {
    name = "web", namespace = "demo",
    # Replace the documentation image with your approved image digest.
    image = "registry.example.com/web@sha256:0000000000000000000000000000000000000000000000000000000000000000",
    port = 8080,
    config = {APP_MODE = "production"},
    secrets = ["web-secrets"],
    ingressHost = "web.example.com", tlsSecret = "web-tls",
    storage = {size = "1Gi", mountPath = "/data"},
    network = {
      ingress = [{
        from = [{namespaceSelector.matchLabels."kubernetes.io/metadata.name" = "kube-system",
          podSelector.matchLabels."app.kubernetes.io/name" = "traefik"}],
        ports = [{protocol = "TCP", port = 8080}],
      }],
      egress = [{
        to = [{namespaceSelector.matchLabels."kubernetes.io/metadata.name" = "kube-system",
          podSelector.matchLabels."k8s-app" = "kube-dns"}],
        ports = [{protocol = "UDP", port = 53}, {protocol = "TCP", port = 53}],
      }],
    },
  },
}
