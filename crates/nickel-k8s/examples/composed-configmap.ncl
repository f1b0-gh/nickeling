let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let c = import "/libraries/nickel-k8s/composition.ncl" in
let settings = c.define {name | String, mode | String} (fun p => [{
    apiVersion = "v1", kind = "ConfigMap",
    metadata = {name = p.name, namespace = "demo"},
    data = {mode = p.mode},
}]) in
c.compose (profiles.kubernetes {}) {
  app = settings {name = "settings", mode = "safe"},
}
