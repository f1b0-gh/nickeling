# Nickeling

Reusable, contract-checked [Nickel](https://nickel-lang.org/) configuration for
Kubernetes and OpenTofu.

Compose configuration with functions, validate it against pinned contracts or
provider schemas, and render Kubernetes manifests or OpenTofu JSON. Use the
compilers independently, or coordinate execution and dependencies with the
optional `nickel-plane` controller.

Nickel defines configuration, Rust provides the tools, and Nix supplies pinned
engines and build recipes.

## Choose a component

| To… | Use… | Result |
| --- | --- | --- |
| Generate Kubernetes/k3s manifests | [nickel-k8s](crates/nickel-k8s/README.md) | A validated JSON `List` |
| Generate OpenTofu configuration | [nickel-tf](crates/nickel-tf/README.md) | `.tf.json` checked against frozen provider schemas |
| Coordinate execution and dependencies | [nickel-plane](crates/nickel-plane/README.md) | Plans, reconciliation and durable operation state |
| Reuse the shared Rust primitives | [nickel-util](crates/nickel-util/README.md) | Bounded files, JSON, processes and confined evaluation |

Both compilers work on their own. `nickel-plane` uses them through optional
adapters; neither compiler depends on the controller.

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

## Try it

The current crates target Linux; pinned engines and examples target x86_64 Linux.
Building requires stable Rust and a C compiler. Evaluating Nickel also requires
Nix, Bubblewrap, `prlimit` and unprivileged user namespaces.

From the repository root:

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

## Installation and resources

Use a source checkout for the commands above. To install a CLI from that
checkout, for example:

```sh
cargo install --path crates/nickel-k8s --locked
```

This installs the executable. Keep the matching checkout for its `nickel/`,
`flake.nix` and `flake.lock` resources, and pass the crate directory as
`--resources`. The OpenTofu compiler also needs an explicitly prepared schema
bundle. Resource directories contain trusted code; project configuration is
evaluated separately in confinement.

Rust consumers can use the libraries with `default-features = false` to omit
Clap; `nickel-util` has no CLI. See the [development guide](docs/development.md)
for resource layout, source pinning and native tool preparation.

## From configuration to execution

`check`, `render` and `lock` validate inputs without running OpenTofu, providers
or project commands. Locks bind the result to the modules, contracts and recipes
used to produce it.

`nickel-plane` handles execution. Its `plan` command contacts targets and records
proposed operations; `apply` requires a specific operation and refuses stale
plans. By default, an instance only observes resources and retains them when
removed from management.

The [container recipes](crates/nickel-plane/containers/README.md) package the
controller as a systemd-nspawn rootfs or a Podman image with Quadlet units.
These are service containers.

The current Kubernetes contracts cover nine resource kinds; CRDs and Secret
objects are rejected. OpenTofu compilation excludes modules and provisioners.
See each component's guide for supported inputs and execution limits.

## Development

All four crates and their resources are in this repository. Cargo writes to
`target/`, using at most two build jobs.

See the [test commands](docs/development.md), [documentation](docs/README.md)
and [development rules](AGENTS.md). Tests use fixtures and pinned engines;
they do not qualify a live cluster or Proxmox infrastructure.

## Project status

Nickeling is under development at `0.1.0`. GitHub is the current publication target.
Real Kubernetes operation and deployment on Scaleway still need validation.
Crates.io publication is deferred until the intended workflows have been tested.

## Dependencies and security advisories

Updates follow the [dependency policy](docs/dependencies.md). A lockfile makes
versions reproducible; it does not establish that they are free of vulnerabilities.

## License

[Apache-2.0](LICENSE). Third-party contracts and schemas retain their licenses
and notices in their respective directories.
