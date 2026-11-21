let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"datastore_id" | Core.Value (Core.NonNull (String)),
"file_name" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"overwrite" | Core.Value (Core.Nullable (Bool)) | optional,
"overwrite_unmanaged" | Core.Value (Core.Nullable (Bool)) | optional,
"reference" | Core.Value (Core.NonNull (String)),
"upload_timeout" | Core.Value (Core.Nullable (Number)) | optional,
})
