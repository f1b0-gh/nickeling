# nickel-k8s

Compile Nickel configurations into Kubernetes or k3s manifests.

The compiler turns a typed configuration into a Kubernetes JSON `List`, validates
resources and locks its inputs. Kubernetes and k3s profiles share the same
application modules. Compilation needs no cluster; deployment is a separate step.

[Contracts and profiles](GUIDE.md) · [Examples](examples/) ·
[Development](../../docs/development.md)

## Render a manifest

From the workspace root, with the Linux/Nix prerequisites in the development guide:

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-k8s#nickel \
  --no-link --no-update-lock-file --no-write-lock-file
cargo run -p nickel-k8s --locked -- render \
  --config "$NICKELING_ROOT/crates/nickel-k8s/examples/configmap.ncl" \
  --resources "$NICKELING_ROOT/crates/nickel-k8s"
```

| Command | Result |
| --- | --- |
| `check` | Validate the configuration and contracts |
| `lock` | Fingerprint modules, contracts, recipes and the engine |
| `render` | Produce deterministic JSON; `--lock` requires the locked inputs |

Replacing an existing file requires an explicit option. `check`, `render` and
`lock` do not run cluster management tools or project commands.

The installed CLI also needs the matching Nickel/Nix resource directory.
See [installation and resources](../../README.md#installation-and-resources);
`cargo install` installs the executable only.

## Supported resources

The pinned contracts cover nine resource types. CRDs and unknown types are
rejected, as are Secret objects: configurations refer to separately created
Secrets by name, without embedding their values.

The compiler does not provision nodes or guarantee that a cluster will admit
or run the resources. For observation and application through a controller,
see [nickel-plane](../nickel-plane/README.md).

Use `default-features = false` for the Rust API without Clap.
The [guide](GUIDE.md) covers versions, limits and resource composition.
