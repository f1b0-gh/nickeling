let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "terraform" "terraform" ({
"input" | Core.Value (Core.Nullable (Dyn)) | optional,
"triggers_replace" | Core.Value (Core.Nullable (Dyn)) | optional,
})
