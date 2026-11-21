let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"pool_id" | Core.Value (Core.NonNull (String)),
"storage_id" | Core.Value (Core.Nullable (String)) | optional,
"vm_id" | Core.Value (Core.Nullable (Number)) | optional,
})
