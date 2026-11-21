let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"cidr" | Core.Value (Core.NonNull (String)),
"dhcp_dns_server" | Core.Value (Core.Nullable (String)) | optional,
"dhcp_range" | Core.Value (Core.Nullable ({
"end_address" | Core.Value (Core.NonNull (String)),
"start_address" | Core.Value (Core.NonNull (String)),
})) | optional,
"dns_zone_prefix" | Core.Value (Core.Nullable (String)) | optional,
"gateway" | Core.Value (Core.Nullable (String)) | optional,
"snat" | Core.Value (Core.Nullable (Bool)) | optional,
"vnet" | Core.Value (Core.NonNull (String)),
})
