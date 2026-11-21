let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"container_id" | Core.Value (Core.Nullable (Number)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.Nullable (String)) | optional,
"vm_id" | Core.Value (Core.Nullable (Number)) | optional,
"cidr" | Core.Bounds 0 0 (Array ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"nomatch" | Core.Value (Core.Nullable (Bool)) | optional,
})) | optional,
})
