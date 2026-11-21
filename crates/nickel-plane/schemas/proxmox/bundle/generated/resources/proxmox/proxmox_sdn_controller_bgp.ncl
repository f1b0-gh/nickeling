let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"asn" | Core.Value (Core.NonNull (Number)),
"bgp_multipath_as_path_relax" | Core.Value (Core.Nullable (Bool)) | optional,
"ebgp" | Core.Value (Core.Nullable (Bool)) | optional,
"ebgp_multihop" | Core.Value (Core.Nullable (Number)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"loopback" | Core.Value (Core.Nullable (String)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"peers" | Core.Value (Core.NonNull (Array (Core.Value (Core.Nullable (String))))),
})
