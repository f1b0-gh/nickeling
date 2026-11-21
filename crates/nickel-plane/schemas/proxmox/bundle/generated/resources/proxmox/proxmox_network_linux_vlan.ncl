let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"address" | Core.Value (Core.Nullable (String)) | optional,
"address6" | Core.Value (Core.Nullable (String)) | optional,
"autostart" | Core.Value (Core.Nullable (Bool)) | optional,
"comment" | Core.Value (Core.Nullable (String)) | optional,
"gateway" | Core.Value (Core.Nullable (String)) | optional,
"gateway6" | Core.Value (Core.Nullable (String)) | optional,
"interface" | Core.Value (Core.Nullable (String)) | optional,
"mtu" | Core.Value (Core.Nullable (Number)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
"reload" | Core.Value (Core.Nullable (Bool)) | optional,
"timeout_reload" | Core.Value (Core.Nullable (Number)) | optional,
"vlan" | Core.Value (Core.Nullable (Number)) | optional,
})
