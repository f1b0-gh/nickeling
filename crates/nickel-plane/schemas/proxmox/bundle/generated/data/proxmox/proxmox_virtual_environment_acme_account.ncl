let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"name" | Core.Value (Core.Nullable (String)) | optional,
})
