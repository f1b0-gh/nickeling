let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"ebtables" | Core.Value (Core.Nullable (Bool)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"forward_policy" | Core.Value (Core.Nullable (String)) | optional,
"id" | Core.Value (Core.Nullable (String)) | optional,
"input_policy" | Core.Value (Core.Nullable (String)) | optional,
"output_policy" | Core.Value (Core.Nullable (String)) | optional,
"log_ratelimit" | Core.Bounds 0 1 (Array ({
"burst" | Core.Value (Core.Nullable (Number)) | optional,
"enabled" | Core.Value (Core.Nullable (Bool)) | optional,
"rate" | Core.Value (Core.Nullable (String)) | optional,
})) | optional,
})
