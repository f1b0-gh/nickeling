let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"privileges" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
"role_id" | Core.Value (Core.NonNull (String)),
})
