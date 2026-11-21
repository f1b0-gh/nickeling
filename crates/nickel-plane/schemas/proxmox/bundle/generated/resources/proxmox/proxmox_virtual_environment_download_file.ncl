let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"checksum" | Core.Value (Core.Nullable (String)) | optional,
"checksum_algorithm" | Core.Value (Core.Nullable (String)) | optional,
"content_type" | Core.Value (Core.NonNull (String)),
"datastore_id" | Core.Value (Core.NonNull (String)),
"decompression_algorithm" | Core.Value (Core.Nullable (String)) | optional,
"file_name" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"overwrite" | Core.Value (Core.Nullable (Bool)) | optional,
"overwrite_unmanaged" | Core.Value (Core.Nullable (Bool)) | optional,
"upload_timeout" | Core.Value (Core.Nullable (Number)) | optional,
"url" | Core.Value (Core.NonNull (String)),
"verify" | Core.Value (Core.Nullable (Bool)) | optional,
})
