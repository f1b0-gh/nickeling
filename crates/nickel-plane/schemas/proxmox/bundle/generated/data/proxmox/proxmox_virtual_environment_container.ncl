let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"id" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"status" | Core.Value (Core.Nullable (String)) | optional,
"template" | Core.Value (Core.Nullable (Bool)) | optional,
"vm_id" | Core.Value (Core.NonNull (Number)),
})
