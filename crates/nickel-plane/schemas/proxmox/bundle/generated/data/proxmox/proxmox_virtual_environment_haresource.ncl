let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"resource_id" | Core.Value (Core.NonNull (String)),
})
