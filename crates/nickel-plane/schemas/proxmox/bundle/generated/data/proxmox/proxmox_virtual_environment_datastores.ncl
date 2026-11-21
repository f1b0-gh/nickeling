let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"datastores" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable ({
"active" | Core.Value (Core.Nullable (Bool)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
"shared" | Core.Value (Core.Nullable (Bool)) | optional,
"space_available" | Core.Value (Core.Nullable (Number)) | optional,
"space_total" | Core.Value (Core.Nullable (Number)) | optional,
"space_used" | Core.Value (Core.Nullable (Number)) | optional,
"space_used_fraction" | Core.Value (Core.Nullable (Number)) | optional,
"type" | Core.Value (Core.NonNull (String)),
}))))) | optional,
"filters" | Core.Value (Core.Nullable ({
"content_types" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"target" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
})
