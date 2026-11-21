let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"type" | Core.Value (Core.Nullable (String)) | optional,
})
