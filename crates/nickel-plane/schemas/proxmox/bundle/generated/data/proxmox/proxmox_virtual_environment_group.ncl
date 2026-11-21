let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"group_id" | Core.Value (Core.NonNull (String)),
"id" | Core.Value (Core.Nullable (String)) | optional,
})
