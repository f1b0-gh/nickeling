let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"on_create" | Core.Value (Core.Nullable (Bool)) | optional,
"on_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
})
