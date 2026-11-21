let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"group_id" | Core.Value (Core.Nullable (String)) | optional,
"path" | Core.Value (Core.NonNull (String)),
"propagate" | Core.Value (Core.Nullable (Bool)) | optional,
"role_id" | Core.Value (Core.NonNull (String)),
"token_id" | Core.Value (Core.Nullable (String)) | optional,
"user_id" | Core.Value (Core.Nullable (String)) | optional,
})
