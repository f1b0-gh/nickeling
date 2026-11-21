let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"content" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"create_base_path" | Core.Value (Core.Nullable (Bool)) | optional,
"create_subdirs" | Core.Value (Core.Nullable (Bool)) | optional,
"disable" | Core.Value (Core.Nullable (Bool)) | optional,
"export" | Core.Value (Core.NonNull (String)),
"id" | Core.Value (Core.NonNull (String)),
"nodes" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"options" | Core.Value (Core.Nullable (String)) | optional,
"preallocation" | Core.Value (Core.Nullable (String)) | optional,
"server" | Core.Value (Core.NonNull (String)),
"snapshot_as_volume_chain" | Core.Value (Core.Nullable (Bool)) | optional,
"backups" | {
"keep_all" | Core.Value (Core.Nullable (Bool)) | optional,
"keep_daily" | Core.Value (Core.Nullable (Number)) | optional,
"keep_hourly" | Core.Value (Core.Nullable (Number)) | optional,
"keep_last" | Core.Value (Core.Nullable (Number)) | optional,
"keep_monthly" | Core.Value (Core.Nullable (Number)) | optional,
"keep_weekly" | Core.Value (Core.Nullable (Number)) | optional,
"keep_yearly" | Core.Value (Core.Nullable (Number)) | optional,
"max_protected_backups" | Core.Value (Core.Nullable (Number)) | optional,
} | optional,
})
