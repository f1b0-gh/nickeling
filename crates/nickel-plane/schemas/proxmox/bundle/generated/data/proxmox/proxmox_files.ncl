let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"content_type" | Core.Value (Core.Nullable (String)) | optional,
"datastore_id" | Core.Value (Core.NonNull (String)),
"file_name_regex" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
})
