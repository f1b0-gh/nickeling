let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
})
