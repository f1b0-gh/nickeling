let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"expiration_date" | Core.Value (Core.Nullable (String)) | optional,
"privileges_separation" | Core.Value (Core.Nullable (Bool)) | optional,
"token_name" | Core.Value (Core.NonNull (String)),
"user_id" | Core.Value (Core.NonNull (String)),
})
