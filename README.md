# Nickeling

Composable [Nickel](https://nickel-lang.org/) configuration and reusable Rust
building blocks for infrastructure tooling.

Use `nickel-util` to build tools that load, snapshot and evaluate configuration
under explicit resource limits. Use the independent Kubernetes and OpenTofu
compilers to turn checked configuration into manifests, or coordinate execution
with the optional `nickel-plane` controller. Nix recipes package the controller
and its engines as service containers.

Nickel defines configuration, Rust provides the tools, and Nix supplies pinned
engines and build recipes.

## Why Nickel?

Infrastructure configuration often starts as a few YAML files. As environments
and services multiply, copied manifests and layers of patches make it harder to
see what is shared, what differs and whether a change is valid.

[Nickel](https://nickel-lang.org/) makes configuration programmable:

- **Reuse with functions.** Define service settings, infrastructure resources or
  image build profiles once, then pass parameters for each variant instead of
  copying configuration.
- **Compose small pieces.** Combine shared configuration, reusable libraries and
  application-specific settings into a complete configuration.
- **Catch mistakes with contracts.** Check parameters and resource shapes before
  rendering. Nickeling supplies pinned Kubernetes contracts and OpenTofu provider
  schemas so validation covers the target configuration as well as your inputs.

The included compilers emit ordinary Kubernetes manifests or OpenTofu JSON,
usable by existing tools. The same approach can provide checked inputs for your
own build tooling. Nickel defines the configuration; consumers decide how to
render it, build artifacts or execute changes.

## Choose a component

| To… | Use… | Result |
| --- | --- | --- |
| Build configuration tooling | [nickel-util](crates/nickel-util/README.md) | Bounded files, snapshots, JSON, processes and confined evaluation |
| Generate Kubernetes/k3s manifests | [nickel-k8s](crates/nickel-k8s/README.md) | A validated JSON `List` |
| Generate OpenTofu configuration | [nickel-tf](crates/nickel-tf/README.md) | `.tf.json` checked against frozen provider schemas |
| Coordinate execution and dependencies | [nickel-plane](crates/nickel-plane/README.md) | Plans, reconciliation and durable operation state |
| Package the controller as a service | [Container recipes](crates/nickel-plane/containers/README.md) | systemd-nspawn rootfs, Podman images and Quadlet units |

Both compilers work on their own. `nickel-plane` uses them through optional
adapters; neither compiler depends on the controller.

## Reuse nickel-util

`nickel-util` is a library for configuration tool authors. It provides bounded
file reads and safe publication, source snapshots and fingerprints, JSON
structural checks, bounded subprocess I/O and confined Nickel evaluation. These
primitives help make inputs reproducible and enforce limits before consuming or
publishing data.

Use it for application configuration, build profiles or other domain-specific
tools: it has no Kubernetes or OpenTofu dependency and no CLI. File and JSON
utilities can be used without running Nickel. Your tool owns its configuration
model and execution policy; the library supplies the shared input-handling and
evaluation machinery. See the [library guide](crates/nickel-util/README.md).

Add the published library to your `Cargo.toml`:

```toml
[dependencies]
nickel-util = "=0.1.0"
```

See [nickel-util on crates.io](https://crates.io/crates/nickel-util/0.1.0) and
its [API reference](https://docs.rs/nickel-util/0.1.0/nickel_util/).

## Container builds

The included [container recipes](crates/nickel-plane/containers/README.md) combine
a checked Nickel profile with pinned Nix packages to assemble a controller
rootfs, including systemd, the selected engines and their resources. Build it
for systemd-nspawn or package it as a Podman image with Quadlet units. Both
OpenTofu-only and combined Kubernetes/OpenTofu variants are available; runtime
state and credentials are supplied separately.

## Compose once, reuse with checked inputs

This function creates a ConfigMap from two string parameters. Nickel checks
the parameters before the compiler validates the resulting Kubernetes resource:

```nickel
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let c = import "/libraries/nickel-k8s/composition.ncl" in
let settings = c.define {name | String, mode | String} (fun p => [{
    apiVersion = "v1", kind = "ConfigMap",
    metadata = {name = p.name, namespace = "demo"},
    data = {mode = p.mode},
}]) in
c.compose (profiles.kubernetes {}) {
  app = settings {name = "settings", mode = "safe"},
}
```

Reuse `settings` with different names and values to compose more resources.
The complete example is
[`composed-configmap.ncl`](crates/nickel-k8s/examples/composed-configmap.ncl).
The `/libraries/` imports refer to libraries mounted by the confined evaluator.

## Installation and resources

All four crates are available at version **0.1.0** on crates.io. Choose the
component you need; Cargo builds its required Rust libraries automatically.

The current crates target Linux; pinned engines and examples target x86_64 Linux.
Building requires stable Rust and a C compiler. Evaluating Nickel also requires
Nix, Bubblewrap, `prlimit` and unprivileged user namespaces.

| Component | Install from crates.io |
| --- | --- |
| [Kubernetes compiler](https://crates.io/crates/nickel-k8s/0.1.0) | `cargo install nickel-k8s --version '=0.1.0' --locked` |
| [OpenTofu compiler](https://crates.io/crates/nickel-tf/0.1.0) | `cargo install nickel-tf --version '=0.1.0' --locked` |
| [Controller with both adapters](https://crates.io/crates/nickel-plane/0.1.0) | `cargo install nickel-plane --version '=0.1.0' --locked` |
| Kubernetes-only controller | `cargo install nickel-plane --version '=0.1.0' --locked --no-default-features --features cli,kubernetes` |
| OpenTofu-only controller | `cargo install nickel-plane --version '=0.1.0' --locked --no-default-features --features cli,opentofu` |

To install from a source checkout instead, use
`cargo install --path crates/nickel-k8s --locked` (or the other crate's path),
keeping the same feature selections.

`nickel-util` is a library, so use it as a Rust dependency rather than with
`cargo install`. It has no dependency on the other Nickeling crates. Each
compiler depends on `nickel-util`; the controller includes only the compilers
for its enabled adapters. See the [standalone library example](crates/nickel-util/README.md#usage).

These commands install the selected executable. Keep the matching checkout or
extract the versioned crate archive for its `nickel/`, `flake.nix` and
`flake.lock` resources, and pass the crate directory as `--resources`.
The [resource installation guide](docs/development.md#cli-installation-and-resources)
shows how to obtain these files without a source checkout. The OpenTofu compiler
also needs an explicitly prepared schema bundle. Resource directories contain trusted code; project configuration is
evaluated separately in confinement.

All four components can also be used as Rust libraries. Select version `=0.1.0`
from crates.io, or pin the Git source to a reviewed full commit. Set
`default-features = false` for `nickel-k8s`, `nickel-tf` and `nickel-plane` to omit
their CLI dependencies. For `nickel-plane`,
select `features = ["kubernetes"]`, `features = ["opentofu"]`, or both; neither
adapter is included with default features disabled unless explicitly selected.
`nickel-util` has no feature flags or CLI dependencies.

API references: [nickel-k8s](https://docs.rs/nickel-k8s/0.1.0/nickel_k8s/),
[nickel-tf](https://docs.rs/nickel-tf/0.1.0/nickel_tf/), and
[nickel-plane](https://docs.rs/nickel-plane/0.1.0/nickel_plane/).

See the [development guide](docs/development.md) for resource layout, source
pinning and native tool preparation.

## Try it

From the root of a source checkout:

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-k8s#nickel \
  --no-link --no-update-lock-file --no-write-lock-file
cargo run -p nickel-k8s --locked -- render \
  --config "$NICKELING_ROOT/crates/nickel-k8s/examples/composed-configmap.ncl" \
  --resources "$NICKELING_ROOT/crates/nickel-k8s"
```

The result is a JSON manifest. No cluster is contacted.
The [OpenTofu example](crates/nickel-tf/README.md) uses `terraform_data` and
needs neither a cloud account nor an external provider.

## From configuration to execution

`check`, `render` and `lock` validate inputs without running OpenTofu, providers
or project commands. Locks bind the result to the modules, contracts and recipes
used to produce it.

`nickel-plane` handles execution. Its `plan` command contacts targets and records
proposed operations; `apply` requires a specific operation and refuses stale
plans. By default, an instance only observes resources and retains them when
removed from management.

The current Kubernetes contracts cover nine resource kinds; CRDs and Secret
objects are rejected. OpenTofu compilation excludes modules and provisioners.
See each component's guide for supported inputs and execution limits.

## Development

All four crates and their resources are in this repository. Cargo writes to
`target/`, using at most two build jobs.

See the [test commands](docs/development.md), [documentation](docs/README.md)
and [development rules](AGENTS.md). Tests use fixtures and pinned engines;
they do not qualify a live cluster or Proxmox infrastructure.

For real Kubernetes API behavior, run `python3 scripts/test-kind.py` with a local
Docker daemon. The [kind testing guide](docs/kubernetes-testing.md) covers the
same disposable cluster driver used in CI, including pinned tools and cleanup.
This checks local API semantics; k3s, production operation and Scaleway need
separate qualification.

## Project status

Nickeling is under development at 0.1.0.

## Dependencies and security advisories

Updates follow the [dependency policy](docs/dependencies.md). A lockfile makes
versions reproducible; it does not establish that they are free of vulnerabilities.

## License

[Apache-2.0](LICENSE). Third-party contracts and schemas retain their licenses
and notices in their respective directories.
