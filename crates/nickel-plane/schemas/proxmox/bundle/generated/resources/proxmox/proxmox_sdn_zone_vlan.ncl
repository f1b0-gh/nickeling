let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"bridge" | Core.Value (Core.NonNull (String)),
"dns" | Core.Value (Core.Nullable (String)) | optional,
"dns_zone" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"ipam" | Core.Value (Core.Nullable (String)) | optional,
"mtu" | Core.Value (Core.Nullable (Number)) | optional,
"nodes" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"reverse_dns" | Core.Value (Core.Nullable (String)) | optional,
})
