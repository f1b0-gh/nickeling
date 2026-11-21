let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"content_type" | Core.Value (Core.Nullable (String)) | optional,
"datastore_id" | Core.Value (Core.NonNull (String)),
"file_mode" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"overwrite" | Core.Value (Core.Nullable (Bool)) | optional,
"timeout_upload" | Core.Value (Core.Nullable (Number)) | optional,
"upload_mode" | Core.Value (Core.Nullable (String)) | optional,
"source_file" | Core.Bounds 0 1 (Array ({
"changed" | Core.Value (Core.Nullable (Bool)) | optional,
"checksum" | Core.Value (Core.Nullable (String)) | optional,
"file_name" | Core.Value (Core.Nullable (String)) | optional,
"insecure" | Core.Value (Core.Nullable (Bool)) | optional,
"min_tls" | Core.Value (Core.Nullable (String)) | optional,
"path" | Core.Value (Core.NonNull (String)),
})) | optional,
"source_raw" | Core.Bounds 0 1 (Array ({
"data" | Core.Value (Core.NonNull (String)),
"file_name" | Core.Value (Core.NonNull (String)),
"resize" | Core.Value (Core.Nullable (Number)) | optional,
})) | optional,
})
