let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"container_id" | Core.Value (Core.Nullable (Number)) | optional,
"dhcp" | Core.Value (Core.Nullable (Bool)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"input_policy" | Core.Value (Core.Nullable (String)) | optional,
"ipfilter" | Core.Value (Core.Nullable (Bool)) | optional,
"log_level_in" | Core.Value (Core.Nullable (String)) | optional,
"log_level_out" | Core.Value (Core.Nullable (String)) | optional,
"macfilter" | Core.Value (Core.Nullable (Bool)) | optional,
"ndp" | Core.Value (Core.Nullable (Bool)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"output_policy" | Core.Value (Core.Nullable (String)) | optional,
"radv" | Core.Value (Core.Nullable (Bool)) | optional,
"vm_id" | Core.Value (Core.Nullable (Number)) | optional,
})
