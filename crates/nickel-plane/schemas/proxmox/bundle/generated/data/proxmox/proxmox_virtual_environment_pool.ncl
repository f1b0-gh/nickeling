let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"pool_id" | Core.Value (Core.NonNull (String)),
})
