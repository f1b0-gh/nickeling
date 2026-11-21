let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"map" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"iommu_group" | Core.Value (Core.Nullable (Number)) | optional,
"node" | Core.Value (Core.NonNull (String)),
"path" | Core.Value (Core.NonNull (String)),
"subsystem_id" | Core.Value (Core.Nullable (String)) | optional,
}))))),
"mediated_devices" | Core.Value (Core.Nullable (Bool)) | optional,
"name" | Core.Value (Core.NonNull (String)),
})
