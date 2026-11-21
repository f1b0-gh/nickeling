let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"csnp_interval" | Core.Value (Core.Nullable (Number)) | optional,
"hello_interval" | Core.Value (Core.Nullable (Number)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"ip6_prefix" | Core.Value (Core.Nullable (String)) | optional,
"ip_prefix" | Core.Value (Core.Nullable (String)) | optional,
})
