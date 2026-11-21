let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"disable" | Core.Value (Core.Nullable (Bool)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"rate" | Core.Value (Core.Nullable (Number)) | optional,
"schedule" | Core.Value (Core.Nullable (String)) | optional,
"target" | Core.Value (Core.NonNull (String)),
"type" | Core.Value (Core.NonNull (String)),
})
