# Real Kubernetes API tests

From the repository root, with Python 3, Nix, Rust and access to a local Docker
daemon:

```sh
python3 scripts/test-kind.py
```

For the same pinned Rust development shell used by CI:

```sh
nix --extra-experimental-features 'nix-command flakes' develop \
  --no-update-lock-file --no-write-lock-file path:./crates/nickel-plane \
  --command python3 scripts/test-kind.py
```

The shorter command uses the Rust toolchain already on your PATH; both commands
use the same pinned kind, kubectl and node image. Each run records tool versions.

The same command runs in the `Kubernetes API (kind)` CI job. It prepares kind
and kubectl from the existing Nix lock and creates a uniquely named disposable
cluster. No global kind installation is needed. The node image is Kubernetes
1.34.11, matching the contracts' 1.34 minor, pinned by digest from the official
[kind 0.33.0 release](https://github.com/kubernetes-sigs/kind/releases/tag/v0.33.0).
The Docker image download is separate from compiler/evaluator execution.

The API is published only on loopback. The driver passes a private kubeconfig
explicitly and does not read or modify the user's Kubernetes context. Bootstrap
operations use kubectl inside the node image, matching the API version; the
host's pinned kubectl only reads the private kubeconfig locally. Tests use a
short-lived service-account token restricted to the dedicated namespace and
verify the server against its explicit CA. Credentials are never printed.

Rust integration tests exercise actual API field ownership, server-side apply,
drift and refusal behavior. They are ignored by ordinary `cargo test` because
they require this explicitly provisioned cluster. Use `--filter TEST_NAME` for a
focused run. These tests qualify local API behavior, not k3s, production
availability, external networking, or workload deployment readiness.

The driver always attempts to remove its own cluster after success, failure or
interruption. It verifies the cluster label before deletion. Existing clusters
and images are preserved. An image first fetched by this run is removed only
when no container uses it, without forcing removal. No broad Docker prune runs.
Kubeconfig and token files are removed when the run finishes. Abrupt host loss or
SIGKILL cannot execute cleanup; the private result directory identifies the
unique cluster for an operator to inspect.

Evidence defaults to `.local/kind/<unique-name>/`; `--output NEW_DIRECTORY`
selects another private directory outside crate resource trees. Keep test source,
the recorded reproduction command, source fingerprints, result summary, PDF
report and relevant logs. A result records whether source files changed during
the run; repeat against stable inputs before treating it as final evidence. Raw cluster
logs remain private and are not uploaded by CI. Public CI output contains only
test outcomes and deliberately selected diagnostic facts, never kubeconfig,
tokens or private keys. Cargo uses at most two jobs, with build outputs outside
Nix source trees. The driver does not delete shared Cargo caches.
