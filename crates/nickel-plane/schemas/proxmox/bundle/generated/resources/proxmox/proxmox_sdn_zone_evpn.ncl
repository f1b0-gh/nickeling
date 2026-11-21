let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"advertise_subnets" | Core.Value (Core.Nullable (Bool)) | optional,
"controller" | Core.Value (Core.NonNull (String)),
"disable_arp_nd_suppression" | Core.Value (Core.Nullable (Bool)) | optional,
"dns" | Core.Value (Core.Nullable (String)) | optional,
"dns_zone" | Core.Value (Core.Nullable (String)) | optional,
"exit_nodes" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"exit_nodes_local_routing" | Core.Value (Core.Nullable (Bool)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"ipam" | Core.Value (Core.Nullable (String)) | optional,
"mtu" | Core.Value (Core.Nullable (Number)) | optional,
"nodes" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"primary_exit_node" | Core.Value (Core.Nullable (String)) | optional,
"reverse_dns" | Core.Value (Core.Nullable (String)) | optional,
"rt_import" | Core.Value (Core.Nullable (String)) | optional,
"vrf_vxlan" | Core.Value (Core.NonNull (Number)),
})
