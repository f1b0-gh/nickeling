let t = import "/libraries/nickel-tf/composition.ncl" in
let tf = import "/libraries/nickel-tf/expressions.ncl" in
let k = import "/libraries/nickel-k8s/composition.ncl" in
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
{
 apiVersion="nickel-plane/v1", id="mixed-demo",
 composition={name="settings-to-cluster",version="1"},
 policy={mode="observe"},
 units={
  infrastructure={backend="opentofu",target="local",schema="local",
   configuration=t.compose {
    storage={resource.terraform_data.settings.input="hello from OpenTofu"},
    exports={output.message.value=tf.ref ["terraform_data","settings","output"]},
   },outputs.message.pointer="/outputs/message",
  },
  application={backend="kubernetes",target="cluster",
   configuration=k.compose (profiles.kubernetes {}) {
    settings=[{apiVersion="v1",kind="ConfigMap",metadata={name="settings",namespace="demo"},data.message="pending"}],
   },
   bindings=[{fromUnit="infrastructure",output="message",to="/resources/0/data/message",condition="applied"}],
  },
 },
}
