let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"cidr" | Core.Value (Core.NonNull (String)),
"comment" | Core.Value (Core.Nullable (String)) | optional,
"container_id" | Core.Value (Core.Nullable (Number)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.Nullable (String)) | optional,
"vm_id" | Core.Value (Core.Nullable (Number)) | optional,
})
