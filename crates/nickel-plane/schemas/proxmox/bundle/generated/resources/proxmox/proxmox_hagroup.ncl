let Core = import "/libraries/nickel-tf/contracts.ncl" in
Core.ResourceFor "proxmox" "proxmox" ({
"comment" | Core.Value (Core.Nullable (String)) | optional,
"group" | Core.Value (Core.NonNull (String)),
"no_failback" | Core.Value (Core.Nullable (Bool)) | optional,
"nodes" | Core.Value (Core.NonNull ({ _ | Core.Value (Core.Nullable (Number)) })),
"restricted" | Core.Value (Core.Nullable (Bool)) | optional,
})
