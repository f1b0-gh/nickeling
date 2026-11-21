let c = import "/libraries/nickel-k8s/composition.ncl" in
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let application = import "/libraries/nickel-k8s/application.ncl" in
let chosen_target = profiles.k3s {networkPolicy=true} in
{
 apiVersion="nickel-plane/v1",id="web-demo",composition={name="web",version="1"},
 policy={mode="observe"},
 units.web={backend="kubernetes",target="cluster",configuration=c.compose chosen_target {
  web=application chosen_target {
   name="web",namespace="demo",
   image="registry.example.com/web@sha256:0000000000000000000000000000000000000000000000000000000000000000",
   port=8080,network={ingress=[],egress=[]},
  },
 }},
}
