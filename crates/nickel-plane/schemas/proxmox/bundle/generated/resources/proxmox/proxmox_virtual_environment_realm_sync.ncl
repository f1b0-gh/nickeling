let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"dry_run" | Core.Value (Core.Nullable (Bool)) | optional,
"enable_new" | Core.Value (Core.Nullable (Bool)) | optional,
"full" | Core.Value (Core.Nullable (Bool)) | optional,
"purge" | Core.Value (Core.Nullable (Bool)) | optional,
"realm" | Core.Value (Core.NonNull (String)),
"remove_vanished" | Core.Value (Core.Nullable (String)) | optional,
"scope" | Core.Value (Core.Nullable (String)) | optional,
})
