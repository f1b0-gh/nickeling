let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "terraform" "terraform" ({
"backend" | Core.Value (Core.NonNull (String)),
"config" | Core.Value (Core.Nullable (Dyn)) | optional,
"defaults" | Core.Value (Core.Nullable (Dyn)) | optional,
"workspace" | Core.Value (Core.Nullable (String)) | optional,
})
