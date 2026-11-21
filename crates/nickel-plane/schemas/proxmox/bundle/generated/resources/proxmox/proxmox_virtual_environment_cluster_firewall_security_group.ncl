let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"container_id" | Core.Value (Core.Nullable (Number)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.Nullable (String)) | optional,
"vm_id" | Core.Value (Core.Nullable (Number)) | optional,
"rule" | Core.Bounds 0 0 (Array ({
"action" | Core.Value (Core.Nullable (String)) | optional,
"comment" | Core.Value (Core.Nullable (String)) | optional,
"dest" | Core.Value (Core.Nullable (String)) | optional,
"dport" | Core.Value (Core.Nullable (String)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"iface" | Core.Value (Core.Nullable (String)) | optional,
"log" | Core.Value (Core.Nullable (String)) | optional,
"macro" | Core.Value (Core.Nullable (String)) | optional,
"proto" | Core.Value (Core.Nullable (String)) | optional,
"security_group" | Core.Value (Core.Nullable (String)) | optional,
"source" | Core.Value (Core.Nullable (String)) | optional,
"sport" | Core.Value (Core.Nullable (String)) | optional,
"type" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
})
