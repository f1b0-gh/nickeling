let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"contact" | Core.Value (Core.NonNull (String)),
"directory" | Core.Value (Core.Nullable (String)) | optional,
"eab_hmac_key" | Core.Value (Core.Nullable (String)) | optional,
"eab_kid" | Core.Value (Core.Nullable (String)) | optional,
"name" | Core.Value (Core.Nullable (String)) | optional,
"tos" | Core.Value (Core.Nullable (String)) | optional,
})
