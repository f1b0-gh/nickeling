let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"asn" | Core.Value (Core.NonNull (Number)),
"fabric" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"peers" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
})
