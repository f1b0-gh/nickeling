# nickel-tf

Compile Nickel configurations into `.tf.json` using frozen provider schemas.

The compiler imports explicitly prepared provider schemas, generates their
Nickel contracts, then validates and renders `.tf.json`. It preserves deferred
OpenTofu expressions and locks the inputs used to produce the result.

[Schemas and expressions](https://github.com/f1b0-gh/nickeling/blob/main/crates/nickel-tf/GUIDE.md) · [Examples](https://github.com/f1b0-gh/nickeling/blob/main/crates/nickel-tf/examples) ·
[Development](https://github.com/f1b0-gh/nickeling/blob/main/docs/development.md)

## Install

After version 0.1.0 is published on crates.io:

```sh
cargo install nickel-tf --version 0.1.0 --locked
```

The executable needs matching trusted Nickel/Nix resources; see
[resource installation](https://github.com/f1b0-gh/nickeling/blob/main/docs/development.md#cli-installation-and-resources).
Source-checkout commands below work before publication as well.

## A local example

From the workspace root, with the Linux/Nix prerequisites installed:

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-tf#nickel \
  --no-link --no-update-lock-file --no-write-lock-file
TF_DEMO=$(mktemp -d)
cargo run -p nickel-tf --locked -- import-schema \
  --schema crates/nickel-tf/examples/local/schema.json \
  --selection crates/nickel-tf/examples/local/selection.json \
  --output "$TF_DEMO/bundle"
cargo run -p nickel-tf --locked -- render \
  --config "$NICKELING_ROOT/crates/nickel-tf/examples/local/main.ncl" \
  --resources "$NICKELING_ROOT/crates/nickel-tf" --bundle "$TF_DEMO/bundle" \
  --output "$TF_DEMO/main.tf.json"
```

The output configures `terraform_data`, which is built into OpenTofu. These
commands do not apply any infrastructure and need no cloud account.

| Command | Result |
| --- | --- |
| `import-schema` | Bundle schemas, selections, contracts and the provider lock |
| `check` | Validate Nickel against the bundle |
| `lock` | Fingerprint inputs and output |
| `render` | Produce `.tf.json`, optionally bound to a lock |

The installed CLI also needs the matching Nickel/Nix resource directory.
See [installation and resources](https://github.com/f1b0-gh/nickeling/blob/main/README.md#installation-and-resources);
`cargo install` installs the executable only.

## Scope

Acquiring providers and running `tofu init`, `validate`, `plan` or `apply` are
separate native steps. The compiler does not run them or resolve versions over
the network. Frozen schemas and their hashes detect changes; they do not, on
their own, establish the provenance of a provider binary.

Modules, remote backends and provisioners are outside the initial scope.
Rendered output may contain values you supply: keep secrets and OpenTofu state
out of version control. Use [nickel-plane](https://github.com/f1b0-gh/nickeling/blob/main/crates/nickel-plane/README.md) to
coordinate execution.

The Rust API stays free of Clap with `default-features = false`.
