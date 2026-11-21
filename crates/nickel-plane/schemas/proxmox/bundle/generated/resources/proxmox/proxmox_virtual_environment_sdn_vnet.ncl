let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"alias" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"isolate_ports" | Core.Value (Core.Nullable (Bool)) | optional,
"tag" | Core.Value (Core.Nullable (Number)) | optional,
"vlan_aware" | Core.Value (Core.Nullable (Bool)) | optional,
"zone" | Core.Value (Core.NonNull (String)),
})
