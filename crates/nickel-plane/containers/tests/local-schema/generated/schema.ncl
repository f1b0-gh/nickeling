let Core = import "/libraries/nickel-tf/contracts.ncl" in
{
"provider" | {
"terraform" | Core.ProviderConfigs (import "providers/terraform.ncl") | optional,
} | optional,
"resource" | {
"terraform_data" | { _ | (import "resources/terraform/terraform_data.ncl") } | optional,
} | optional,
"data" | {
"terraform_remote_state" | { _ | (import "data/terraform/terraform_remote_state.ncl") } | optional,
} | optional,
"variable" | { _ | Core.Variable } | optional,
"locals" | { _ | Core.Value Dyn } | optional,
"output" | { _ | Core.Output } | optional,
"terraform" | Core.Terraform | optional,
}
