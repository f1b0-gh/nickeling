let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"content_type" | Core.Value (Core.NonNull (String)),
"datastore_id" | Core.Value (Core.NonNull (String)),
"file_name" | Core.Value (Core.NonNull (String)),
"node_name" | Core.Value (Core.NonNull (String)),
})
