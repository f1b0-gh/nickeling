let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"handle" | Core.Value (Core.NonNull (String)),
"node" | Core.Value (Core.NonNull (String)),
})
