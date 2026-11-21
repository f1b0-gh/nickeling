let tf = import "/libraries/nickel-tf/expressions.ncl" in
{
  resource.terraform_data = {
    empty = {},
    settings.input = {
      message = "local OpenTofu fixture",
      literal_template = "$" ++ "{leave_this_literal}",
      empty_map = {},
      "${literal_key}" = "key preserved",
    },
    copy.input = tf.ref ["terraform_data", "settings", "output"],
  },
  output.settings.value = tf.ref ["terraform_data", "copy", "output"],
}
