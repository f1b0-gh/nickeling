let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"account" | Core.Value (Core.NonNull (String)),
"domains" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable ({
"alias" | Core.Value (Core.Nullable (String)) | optional,
"domain" | Core.Value (Core.NonNull (String)),
"plugin" | Core.Value (Core.Nullable (String)) | optional,
}))))),
"force" | Core.Value (Core.Nullable (Bool)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
})
