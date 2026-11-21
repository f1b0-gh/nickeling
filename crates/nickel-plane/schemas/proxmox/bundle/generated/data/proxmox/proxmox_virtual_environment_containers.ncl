let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.Nullable (String)) | optional,
"tags" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"filter" | Core.Bounds 0 0 (Array ({
"name" | Core.Value (Core.NonNull (String)),
"regex" | Core.Value (Core.Nullable (Bool)) | optional,
"values" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
})) | optional,
})
