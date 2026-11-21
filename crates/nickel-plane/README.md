# nickel-plane

A controller for Kubernetes and OpenTofu units described in Nickel.

nickel-plane maps instances to explicit targets, orders their dependencies and
keeps durable operation state. It uses nickel-k8s and nickel-tf through optional
adapters. Both compilers remain usable without the controller.

[Controller guide](GUIDE.md) · [Examples](examples/) ·
[Containers](containers/README.md) · [Development](../../docs/development.md)

## Build

From the workspace root:

```sh
cargo build -p nickel-plane --locked
cargo run -p nickel-plane --locked -- --help
# OpenTofu only:
cargo build -p nickel-plane --locked --no-default-features --features cli,opentofu
# Library only, without Clap or adapters:
cargo build -p nickel-plane --locked --no-default-features --lib
```

An instance declares its units, targets, dependencies, outputs and management
policy. The target catalog supplies identities and tool paths; credentials stay
in separate private files. Follow the [local setup](GUIDE.md#build-and-local-use)
for a complete example that prepares a schema, instance and catalog, then checks
and renders a local unit without applying infrastructure. An installed CLI still
needs its matching [resource directories](../../README.md#installation-and-resources).

## Workflow

| Step | Effect |
| --- | --- |
| `check`, `render`, `lock` | Confined compilation and validation |
| `plan` | Observe targets and record proposed operations |
| `apply --plan OPERATION` | Execute a specific operation, refusing it if stale |
| `reconcile --once` / `serve` | Converge according to the instance's explicit policy |
| `status` | Read public status |
| `remove` | Stop managing resources; retain them by default |

The defaults are `observe` and `retain`. Deletion and replacement require the
corresponding permissions. Interrupted operations remain recorded for recovery;
existing state is never adopted implicitly.

## Deployment and limits

The [Nix container recipes](containers/README.md) package the controller for
systemd-nspawn or rootless Podman/Quadlet. They produce service containers with
the selected engines and resources. The `deploy/Containerfile` template is
another option and requires you to select pinned base images explicitly.

Tests use fixtures and pinned native tools. They do not qualify a live cluster,
a Proxmox VM or distributed leader election. The composition model draws on
Crossplane, but this controller does not implement its APIs, providers or packages.
