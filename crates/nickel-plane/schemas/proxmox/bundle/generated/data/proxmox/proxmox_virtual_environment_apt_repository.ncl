let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"file_path" | Core.Value (Core.NonNull (String)),
"index" | Core.Value (Core.NonNull (Number)),
"node" | Core.Value (Core.NonNull (String)),
})
