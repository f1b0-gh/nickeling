let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"auto_rebalance" | Core.Value (Core.Nullable (Bool)) | optional,
"comment" | Core.Value (Core.Nullable (String)) | optional,
"failback" | Core.Value (Core.Nullable (Bool)) | optional,
"group" | Core.Value (Core.Nullable (String)) | optional,
"max_relocate" | Core.Value (Core.Nullable (Number)) | optional,
"max_restart" | Core.Value (Core.Nullable (Number)) | optional,
"resource_id" | Core.Value (Core.NonNull (String)),
"state" | Core.Value (Core.Nullable (String)) | optional,
"type" | Core.Value (Core.Nullable (String)) | optional,
})
