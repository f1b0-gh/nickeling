let c = import "/libraries/nickel-tf/composition.ncl" in
let tf = import "/libraries/nickel-tf/expressions.ncl" in
{
 apiVersion="nickel-plane/v1", id="local-demo",
 composition={name="local-settings",version="1"},
 # Change to manage only after reviewing a plan.
 policy={mode="observe"},
 units.settings={backend="opentofu",target="local",schema="local",
  configuration=c.compose {
   storage={resource.terraform_data.settings.input="hello from OpenTofu"},
   exports={output.message.value=tf.ref ["terraform_data","settings","output"]},
  },
  outputs.message.pointer="/outputs/message",
 },
}
