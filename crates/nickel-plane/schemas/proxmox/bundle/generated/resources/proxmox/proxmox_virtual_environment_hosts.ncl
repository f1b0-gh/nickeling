let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"entry" | Core.Bounds 1 0 (Array ({
"address" | Core.Value (Core.NonNull (String)),
"hostnames" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
})),
})
