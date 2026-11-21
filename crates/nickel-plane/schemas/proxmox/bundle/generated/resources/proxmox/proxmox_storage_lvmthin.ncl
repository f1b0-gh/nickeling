let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"content" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"disable" | Core.Value (Core.Nullable (Bool)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"nodes" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"thin_pool" | Core.Value (Core.NonNull (String)),
"volume_group" | Core.Value (Core.NonNull (String)),
})
