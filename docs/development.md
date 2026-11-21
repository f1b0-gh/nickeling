# Developing Nickeling

Run commands from the repository root. Use stable Rust, a C compiler and at most
two Cargo jobs, as configured in `.cargo/config.toml`. The default `target/`
directory is outside the individual crates' Nix flake sources.

## Prepare native tools

Evaluation and native tests require Linux, Nix, Bubblewrap, `prlimit` and
unprivileged user namespaces. The pinned flakes target x86_64 Linux. Acquiring
the tools is an explicit step, separate from compilation and offline validation.

The Kubernetes TLS fixtures also require OpenSSL on `PATH` and permission to
bind loopback sockets. The pinned `nickel-plane` and `nickel-tf` default
development shells include OpenSSL; CI uses the latter shell. These tests launch
temporary local servers using synthetic test certificates and keys, and stop
them when the fixture exits. They never use host credentials or a live cluster.

```sh
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-tf#nickel \
  path:./crates/nickel-tf#opentofu \
  path:./crates/nickel-tf#opentofu-local \
  --no-link --no-update-lock-file --no-write-lock-file
export NICKEL_BIN="$(nix --extra-experimental-features 'nix-command flakes' eval --raw --no-update-lock-file --no-write-lock-file path:./crates/nickel-tf#nickel.outPath)/bin/nickel"
export TOFU_BIN="$(nix --extra-experimental-features 'nix-command flakes' eval --raw --no-update-lock-file --no-write-lock-file path:./crates/nickel-tf#opentofu.outPath)/bin/tofu"
export TOFU_LOCAL_BIN="$(nix --extra-experimental-features 'nix-command flakes' eval --raw --no-update-lock-file --no-write-lock-file path:./crates/nickel-tf#opentofu-local.outPath)/bin/tofu"
```

## Required checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo check --workspace --lib --no-default-features --locked
cargo check -p nickel-plane --no-default-features --features cli,opentofu --locked
cargo check -p nickel-plane --no-default-features --features cli,kubernetes --locked
```

Select checks for the change and its concrete risks. Reuse evidence when its
relevant inputs are unchanged; do not repeat checks without a reason. Record
passes, failures, blocked checks and coverage gaps. A confinement failure must
remain a refusal; do not weaken the sandbox to make a test pass.

OpenTofu tests use disposable local state and explicitly prepared providers.
Ordinary Kubernetes tests use fixtures. Run `python3 scripts/test-kind.py` to
exercise the real API in an owned disposable kind cluster; see the
[local and CI testing guide](kubernetes-testing.md) for prerequisites, evidence
and cleanup. These tests do not qualify k3s or production deployments. Container tests
have a separate [qualification procedure](../crates/nickel-plane/containers/GUIDE.md).
An unvalidated audit lead is not a confirmed vulnerability.

## Continuous integration

[GitHub Actions](../.github/workflows/ci.yml) runs the checks above on pushes to
`main`, pull requests and manual dispatch. It uses Ubuntu 22.04 x86_64, the
existing Nix-locked development shell and explicitly prepared native tools.
Cargo dependencies are fetched before offline validation; outputs stay under
`RUNNER_TEMP`, with at most two Cargo jobs and two test threads. Confinement
failures fail the job rather than disabling protection.

The workflow has read-only repository access and no publication or deployment
steps. Checkout and Nix installation actions are pinned to full commit IDs.
It covers local fixtures, not live Kubernetes, Scaleway or container VM
qualification. Automated advisory scanning and the exact-version divergence
baseline check are not yet wired into CI; passing this workflow does not
establish their coverage.

The separate [Kubernetes API workflow](../.github/workflows/kind.yml) runs the
same kind driver as local development. Its `Kubernetes API (kind)` job checks
real API behavior using a digest-pinned Kubernetes 1.34 node image, with an
explicit private kubeconfig and automatic cleanup of the owned cluster. It does
not deploy to an external cluster or establish Scaleway readiness.

## Release readiness

All four crates are preparing for a first crates.io release at 0.1.0. The kind
suite provides the initial real Kubernetes API validation for that release;
Scaleway deployment remains a separate validation milestone.

Before publishing, require the native and kind CI jobs to pass on the release
revision and verify the actual Cargo archives from a clean checkout:

```sh
cargo package --workspace --all-features --locked
cargo publish --workspace --dry-run --locked
```

Use the pinned development shell and the existing Cargo lockfile. Workspace
packaging builds the extracted archives with registry-style dependencies, so it
also checks that local sibling paths are not required to compile the release.
CI runs archive verification after its source checks. A dry run does not upload.

Publish `nickel-util` first, then `nickel-k8s` and `nickel-tf`, and finally
`nickel-plane`; each dependent crate requires the preceding 0.1.0 packages to
be available in the registry. Record the source revision and archive hashes.
Publishing requires a crates.io account with a verified email and a Cargo
credential configured outside the repository.

## CLI installation and resources

`cargo install --path crates/nickel-k8s --locked` installs the executable, not
its runtime resources. The same distinction applies to `nickel-tf` and
`nickel-plane`. Keep the matching source checkout or explicitly package these
resources from a recorded source revision:

| CLI input | Source contents |
| --- | --- |
| `--resources` | That crate's `nickel/`, `flake.nix` and `flake.lock` |
| `nickel-tf --bundle` | A frozen schema bundle produced by `import-schema` |
| `nickel-plane --tf-resources` | The `nickel-tf` resource directory |
| `nickel-plane --k8s-resources` | The `nickel-k8s` resource directory |
| `nickel-plane --schema NAME=PATH` | A frozen schema bundle for that name |

The crates.io archives include these resources, examples and license notices.
After publication, a CLI can be installed with, for example,
`cargo install nickel-k8s --version 0.1.0 --locked`. To obtain its matching
resources without a repository checkout:

```sh
curl --fail --location https://crates.io/api/v1/crates/nickel-k8s/0.1.0/download \
  --output nickel-k8s-0.1.0.crate
tar -xzf nickel-k8s-0.1.0.crate
K8S_RESOURCES="$(cd nickel-k8s-0.1.0 && pwd -P)"
# Pass --resources "$K8S_RESOURCES" to nickel-k8s.
```

Use the corresponding archive for `nickel-tf` or `nickel-plane`; the controller
also needs the compiler resources for each enabled adapter. Native tools still
need explicit preparation. Container builds use the full source checkout and
its workspace lockfile, rather than an isolated controller archive.

Pass normalized absolute resource paths without symlinks or `..` components.
These are trusted Nickel/Nix resources; project modules are staged separately
for confined evaluation. Keep schema bundles, catalogs and generated outputs
outside the directory containing the project's entry `.ncl` file.

Provider schemas, upstream contracts, notices and fixture locks are versioned
build and test inputs. Generated outputs, runtime state and private receipts
stay outside source control. Preserve Cargo and Nix locks; follow the
[dependency policy](dependencies.md) for changes.

## Rust consumers

After publication, select `nickel-util = "=0.1.0"` (or the other crate's name)
in a consumer's dependencies. The workspace uses versioned local paths between
its four crates; Cargo replaces those paths with registry dependencies when
packaging. For an external consumer before a registry release, select a full Git commit or a versioned,
verifiable source archive. An exact `=0.1.0` requirement alone does not identify
an unpublished source revision; an implicit sibling checkout is not portable.

The compilers and controller expose libraries. Set `default-features = false`
to omit Clap; enable the controller's `opentofu` or `kubernetes` adapter explicitly
when needed. `nickel-util` has no CLI feature or Clap dependency. Evaluation APIs
still need the native tools and resources above; JSON and file primitives do not
run Nickel.

## Experiment retention

Code is the primary retained asset. Before proposing cleanup of an experiment,
integrate or retain maintained source, unique changes, manifests, lockfiles and
irreplaceable inputs outside the proposed deletion.

Generated outputs, intermediates and raw results may be proposed for cleanup
when the retained code has both a usable reproduction recipe and a PDF report.
The recipe records commands, toolchain, dependency versions, configuration and
inputs. The report records findings and limitations and links to the code and
recipe. At handoff, identify all three paths and state which reproduction checks
ran. A PDF alone does not establish reproducibility; missing prerequisites need
a concrete follow-up rather than indefinite retention of all outputs.

Keep raw material only if it is irreplaceable, cannot be reproduced or is needed
for an explicit project obligation; record the reason. Current validation and
release evidence must still be retained as required. Routine build caches need
source and rebuild inputs, not a separate PDF. Eligibility never authorizes
deletion: preserve active work and follow the user's cleanup selection and
applicable deletion checks.

Before independent distribution, replace absolute local paths and implicit
sibling checkouts with explicit, versioned acquisition or packaging. Record any
remaining cross-project dependency and its portability work. Do not silently
change dependency sources or claim an independent checkout was verified without
running that check.
