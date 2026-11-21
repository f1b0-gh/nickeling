# Developing Nickeling

Run commands from the repository root. Use stable Rust, a C compiler and at most
two Cargo jobs, as configured in `.cargo/config.toml`. The default `target/`
directory is outside the individual crates' Nix flake sources.

## Prepare native tools

Evaluation and native tests require Linux, Nix, Bubblewrap, `prlimit` and
unprivileged user namespaces. The pinned flakes target x86_64 Linux. Acquiring
the tools is an explicit step, separate from compilation and offline validation.

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
Kubernetes tests use fixtures and do not qualify a live cluster. Container tests
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

## Release readiness

The current distribution target is the GitHub source repository. Real Kubernetes
operation and deployment on Scaleway remain to be validated. Local fixture,
compiler and container checks do not establish that readiness. Crates.io
publication is deferred until the intended workflows have been exercised.

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

Pass normalized absolute resource paths without symlinks or `..` components.
These are trusted Nickel/Nix resources; project modules are staged separately
for confined evaluation. Keep schema bundles, catalogs and generated outputs
outside the directory containing the project's entry `.ncl` file.

Provider schemas, upstream contracts, notices and fixture locks are versioned
build and test inputs. Generated outputs, runtime state and private receipts
stay outside source control. Preserve Cargo and Nix locks; follow the
[dependency policy](dependencies.md) for changes.

## Rust consumers

The workspace uses versioned local paths between its four crates. For an external
consumer before a registry release, select a full Git commit or a versioned,
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
