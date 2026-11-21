let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"fabric_id" | Core.Value (Core.NonNull (String)),
"node_id" | Core.Value (Core.NonNull (String)),
})
