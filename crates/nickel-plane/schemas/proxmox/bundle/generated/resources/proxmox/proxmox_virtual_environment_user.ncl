let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"email" | Core.Value (Core.Nullable (String)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"expiration_date" | Core.Value (Core.Nullable (String)) | optional,
"first_name" | Core.Value (Core.Nullable (String)) | optional,
"groups" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"keys" | Core.Value (Core.Nullable (String)) | optional,
"last_name" | Core.Value (Core.Nullable (String)) | optional,
"password" | Core.Value (Core.Nullable (String)) | optional,
"user_id" | Core.Value (Core.NonNull (String)),
"acl" | Core.Bounds 0 0 (Array ({
"path" | Core.Value (Core.NonNull (String)),
"propagate" | Core.Value (Core.Nullable (Bool)) | optional,
"role_id" | Core.Value (Core.NonNull (String)),
})) | optional,
})
