let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"domain" | Core.Value (Core.NonNull (String)),
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"servers" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
})
