let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"file_path" | Core.Value (Core.NonNull (String)),
"index" | Core.Value (Core.NonNull (Number)),
"node" | Core.Value (Core.NonNull (String)),
})
