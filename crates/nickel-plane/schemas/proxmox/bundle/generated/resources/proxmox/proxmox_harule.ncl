let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"affinity" | Core.Value (Core.Nullable (String)) | optional,
"comment" | Core.Value (Core.Nullable (String)) | optional,
"disable" | Core.Value (Core.Nullable (Bool)) | optional,
"nodes" | Core.Value (Core.Nullable ({ _ | Core.Value (Core.Nullable (Number)) })) | optional,
"resources" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
"rule" | Core.Value (Core.NonNull (String)),
"strict" | Core.Value (Core.Nullable (Bool)) | optional,
"type" | Core.Value (Core.NonNull (String)),
})
