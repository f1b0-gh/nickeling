let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"flags" | Core.Value (Core.Nullable (Array (Core.Value (Core.Nullable (String))))) | optional,
"guest_phys_bits" | Core.Value (Core.Nullable (Number)) | optional,
"hidden" | Core.Value (Core.Nullable (Bool)) | optional,
"hv_vendor_id" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.NonNull (String)),
"level" | Core.Value (Core.Nullable (Number)) | optional,
"phys_bits" | Core.Value (Core.Nullable (String)) | optional,
"reported_model" | Core.Value (Core.NonNull (String)),
})
