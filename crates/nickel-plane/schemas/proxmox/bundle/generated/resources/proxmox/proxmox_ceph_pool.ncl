let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"add_storages" | Core.Value (Core.Nullable (Bool)) | optional,
"application" | Core.Value (Core.Nullable (String)) | optional,
"crush_rule" | Core.Value (Core.Nullable (String)) | optional,
"erasure_coding" | Core.Value (Core.Nullable (String)) | optional,
"force_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
"min_size" | Core.Value (Core.Nullable (Number)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
"pg_autoscale_mode" | Core.Value (Core.Nullable (String)) | optional,
"pg_num" | Core.Value (Core.Nullable (Number)) | optional,
"pg_num_min" | Core.Value (Core.Nullable (Number)) | optional,
"remove_ecprofile" | Core.Value (Core.Nullable (Bool)) | optional,
"remove_storages" | Core.Value (Core.Nullable (Bool)) | optional,
"size" | Core.Value (Core.Nullable (Number)) | optional,
"target_size" | Core.Value (Core.Nullable (String)) | optional,
"target_size_ratio" | Core.Value (Core.Nullable (Number)) | optional,
})
