let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"check_node" | Core.Value (Core.Nullable (String)) | optional,
"type" | Core.Value (Core.NonNull (String)),
})
