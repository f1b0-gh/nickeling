let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"map" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"node" | Core.Value (Core.NonNull (String)),
"path" | Core.Value (Core.Nullable (String)) | optional,
}))))),
"name" | Core.Value (Core.NonNull (String)),
})
