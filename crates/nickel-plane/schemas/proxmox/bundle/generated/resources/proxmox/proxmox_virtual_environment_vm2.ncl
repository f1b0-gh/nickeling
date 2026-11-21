let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"cdrom" | Core.Value (Core.Nullable ({ _ | Core.Value (Core.Nullable ({
"file_id" | Core.Value (Core.Nullable (String)) | optional,
})) })) | optional,
"cpu" | Core.Value (Core.Nullable ({
"affinity" | Core.Value (Core.Nullable (String)) | optional,
"architecture" | Core.Value (Core.Nullable (String)) | optional,
"cores" | Core.Value (Core.Nullable (Number)) | optional,
"flags" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"limit" | Core.Value (Core.Nullable (Number)) | optional,
"numa" | Core.Value (Core.Nullable (Bool)) | optional,
"sockets" | Core.Value (Core.Nullable (Number)) | optional,
"type" | Core.Value (Core.Nullable (String)) | optional,
"units" | Core.Value (Core.Nullable (Number)) | optional,
"vcpus" | Core.Value (Core.Nullable (Number)) | optional,
})) | optional,
"delete_unreferenced_disks_on_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
"description" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (Number)) | optional,
"name" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"purge_on_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
"rng" | Core.Value (Core.Nullable ({
"max_bytes" | Core.Value (Core.Nullable (Number)) | optional,
"period" | Core.Value (Core.Nullable (Number)) | optional,
"source" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
"stop_on_destroy" | Core.Value (Core.Nullable (Bool)) | optional,
"tags" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"template" | Core.Value (Core.Nullable (Bool)) | optional,
"timeouts" | Core.Value (Core.Nullable ({
"create" | Core.Value (Core.Nullable (String)) | optional,
"delete" | Core.Value (Core.Nullable (String)) | optional,
"read" | Core.Value (Core.Nullable (String)) | optional,
"update" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
"vga" | Core.Value (Core.Nullable ({
"clipboard" | Core.Value (Core.Nullable (String)) | optional,
"memory" | Core.Value (Core.Nullable (Number)) | optional,
"type" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
})
