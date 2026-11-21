let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"node_name" | Core.Value (Core.NonNull (String)),
"on_create" | Core.Value (Core.Nullable (Bool)) | optional,
"on_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
"timeout_reload" | Core.Value (Core.Nullable (Number)) | optional,
"triggers" | Core.Value (Core.Nullable ({ _ | Core.Value (Core.Nullable (String)) })) | optional,
})
