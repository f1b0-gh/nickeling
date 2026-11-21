let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"area" | Core.Value (Core.NonNull (String)),
"id" | Core.Value (Core.NonNull (String)),
"ip_prefix" | Core.Value (Core.NonNull (String)),
})
