let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"address" | Core.Value (Core.Nullable (String)) | optional,
"address6" | Core.Value (Core.Nullable (String)) | optional,
"autostart" | Core.Value (Core.Nullable (Bool)) | optional,
"bond_mode" | Core.Value (Core.Nullable (String)) | optional,
"bond_primary" | Core.Value (Core.Nullable (String)) | optional,
"bond_xmit_hash_policy" | Core.Value (Core.Nullable (String)) | optional,
"comment" | Core.Value (Core.Nullable (String)) | optional,
"gateway" | Core.Value (Core.Nullable (String)) | optional,
"gateway6" | Core.Value (Core.Nullable (String)) | optional,
"mtu" | Core.Value (Core.Nullable (Number)) | optional,
"name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
"reload" | Core.Value (Core.Nullable (Bool)) | optional,
"slaves" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
"timeout_reload" | Core.Value (Core.Nullable (Number)) | optional,
})
