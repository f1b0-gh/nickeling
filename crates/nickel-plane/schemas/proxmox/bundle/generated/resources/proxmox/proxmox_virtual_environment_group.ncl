let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"group_id" | Core.Value (Core.NonNull (String)),
"id" | Core.Value (Core.Nullable (String)) | optional,
"acl" | Core.Bounds 0 0 (Array ({
"path" | Core.Value (Core.NonNull (String)),
"propagate" | Core.Value (Core.Nullable (Bool)) | optional,
"role_id" | Core.Value (Core.NonNull (String)),
})) | optional,
})
