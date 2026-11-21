let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"group" | Core.Value (Core.NonNull (String)),
})
