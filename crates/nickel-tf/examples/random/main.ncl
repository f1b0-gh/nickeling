let tf = import "/libraries/nickel-tf/expressions.ncl" in
{
  resource.random_id.fixture = {
    byte_length = 4,
    keepers = { purpose = "local compiler qualification" },
  },
  resource.random_pet.empty = {},
  output.hex.value = tf.ref ["random_id", "fixture", "hex"],
}
