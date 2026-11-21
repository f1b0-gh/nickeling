let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"pool_id" | Core.Value (Core.NonNull (String)),
})
