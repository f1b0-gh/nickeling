let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"fabric_id" | Core.Value (Core.NonNull (String)),
"interface_names" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
"ip" | Core.Value (Core.Nullable (String)) | optional,
"ip6" | Core.Value (Core.Nullable (String)) | optional,
"node_id" | Core.Value (Core.NonNull (String)),
})
