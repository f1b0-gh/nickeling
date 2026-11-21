let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"log_level_forward" | Core.Value (Core.Nullable (String)) | optional,
"log_level_in" | Core.Value (Core.Nullable (String)) | optional,
"log_level_out" | Core.Value (Core.Nullable (String)) | optional,
"ndp" | Core.Value (Core.Nullable (Bool)) | optional,
"nf_conntrack_max" | Core.Value (Core.Nullable (Number)) | optional,
"nf_conntrack_tcp_timeout_established" | Core.Value (Core.Nullable (Number)) | optional,
"nftables" | Core.Value (Core.Nullable (Bool)) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"nosmurfs" | Core.Value (Core.Nullable (Bool)) | optional,
"smurf_log_level" | Core.Value (Core.Nullable (String)) | optional,
"tcp_flags_log_level" | Core.Value (Core.Nullable (String)) | optional,
})
