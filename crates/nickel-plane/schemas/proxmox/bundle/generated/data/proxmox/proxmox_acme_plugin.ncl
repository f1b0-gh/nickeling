let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"plugin" | Core.Value (Core.NonNull (String)),
})
