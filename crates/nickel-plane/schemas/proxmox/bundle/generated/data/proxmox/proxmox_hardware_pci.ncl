let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.DataFor "proxmox" "proxmox" ({
"filters" | Core.Value (Core.Nullable ({
"class" | Core.Value (Core.Nullable (String)) | optional,
"device_id" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"vendor_id" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
"node_name" | Core.Value (Core.NonNull (String)),
"pci_class_blacklist" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
})
