let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"add_storage" | Core.Value (Core.Nullable (Bool)) | optional,
"ashift" | Core.Value (Core.Nullable (Number)) | optional,
"cleanup_config" | Core.Value (Core.Nullable (Bool)) | optional,
"cleanup_disks" | Core.Value (Core.Nullable (Bool)) | optional,
"compression" | Core.Value (Core.Nullable (String)) | optional,
"devices" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
"draid_config" | Core.Value (Core.Nullable ({
"data" | Core.Value (Core.NonNull (Number)),
"spares" | Core.Value (Core.NonNull (Number)),
})) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
"raidlevel" | Core.Value (Core.NonNull (String)),
})
