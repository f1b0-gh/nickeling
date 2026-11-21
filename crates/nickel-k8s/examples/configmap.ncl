let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
{
  target = profiles.kubernetes {},
  resources = [{
    apiVersion = "v1", kind = "ConfigMap",
    metadata = {name = "settings", namespace = "demo"},
    data = {mode = "safe"},
  }],
}
