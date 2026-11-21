let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"node_name" | Core.Value (Core.NonNull (String)),
})
