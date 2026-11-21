let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.NonNull (Number)),
"node_name" | Core.Value (Core.NonNull (String)),
"timeouts" | Core.Value (Core.Nullable ({
"read" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
})
