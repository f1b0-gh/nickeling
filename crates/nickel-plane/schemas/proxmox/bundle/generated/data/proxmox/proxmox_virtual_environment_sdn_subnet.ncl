let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"cidr" | Core.Value (Core.NonNull (String)),
"dhcp_range" | Core.Value (Core.Nullable ({
})) | optional,
"vnet" | Core.Value (Core.NonNull (String)),
})
