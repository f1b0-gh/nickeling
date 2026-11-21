let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"certificate" | Core.Value (Core.NonNull (String)),
"certificate_chain" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"overwrite" | Core.Value (Core.Nullable (Bool)) | optional,
"private_key" | Core.Value (Core.NonNull (String)),
})
