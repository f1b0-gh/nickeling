let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"map" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable ({
"node" | Core.Value (Core.NonNull (String)),
"path" | Core.Value (Core.NonNull (String)),
}))))),
"name" | Core.Value (Core.NonNull (String)),
})
