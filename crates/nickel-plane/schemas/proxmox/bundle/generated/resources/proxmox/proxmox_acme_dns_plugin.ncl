let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"api" | Core.Value (Core.NonNull (String)),
"data" | Core.Value (Core.Nullable ({ _ | Core.Value (Core.Nullable (String)) })) | optional,
"data_wo" | Core.Value (Core.Nullable ({ _ | Core.Value (Core.Nullable (String)) })) | optional,
"data_wo_version" | Core.Value (Core.Nullable (Number)) | optional,
"digest" | Core.Value (Core.Nullable (String)) | optional,
"disable" | Core.Value (Core.Nullable (Bool)) | optional,
"plugin" | Core.Value (Core.NonNull (String)),
"validation_delay" | Core.Value (Core.Nullable (Number)) | optional,
})
