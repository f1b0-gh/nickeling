# nickel-plane

A standalone Rust controller for typed Nickel compositions. It coordinates
optional Kubernetes and OpenTofu units, preserving each backend's native state,
ownership and readiness semantics. Composition functions and named components
live in `nickel-k8s` and `nickel-tf`; neither compiler depends on this controller.
The design borrows composition, dependency and reconciliation concepts from
Crossplane. It does not implement Crossplane APIs, providers or packages.

The initial implementation has **local-fixture qualification**. Tests use the
actual pinned Nickel and OpenTofu, synthetic recovery backends, and a disposable
Kubernetes HTTP fixture. No live cluster or Proxmox VM has been qualified. The
Proxmox example is an offline schema/render/native-validation example.

## Build and local use

Run these commands from the workspace root on x86_64 Linux, with Rust, a C
compiler, Nix, Bubblewrap and `prlimit`. Unprivileged user namespaces are required.
Use normalized absolute resource paths without symlinks or `..` components.

The local example uses OpenTofu's built-in `terraform_data`. Prepare the pinned
Nickel 1.18.0 and OpenTofu 1.13.2 engines, a schema bundle and a target catalog:

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-plane#nickel path:./crates/nickel-plane#opentofu \
  --no-link --no-update-lock-file --no-write-lock-file
TOFU_BIN="$(nix --extra-experimental-features 'nix-command flakes' eval --raw \
  --no-update-lock-file --no-write-lock-file path:./crates/nickel-plane#opentofu.outPath)/bin/tofu"
PLANE_RESOURCES="$NICKELING_ROOT/crates/nickel-plane"
TF_RESOURCES="$NICKELING_ROOT/crates/nickel-tf"
PLANE_DEMO=$(mktemp -d)
mkdir "$PLANE_DEMO/instance"
cp "$PLANE_RESOURCES/examples/local/main.ncl" "$PLANE_DEMO/instance/main.ncl"

cargo run -p nickel-tf --locked -- import-schema \
  --schema "$TF_RESOURCES/examples/local/schema.json" \
  --selection "$TF_RESOURCES/examples/local/selection.json" \
  --output "$PLANE_DEMO/schema"
cat > "$PLANE_DEMO/targets.json" <<EOF
{"version":1,"targets":{"local":{"backend":"opentofu",
  "config":{"binary":"$TOFU_BIN","stack":"local-demo"}}}}
EOF

cargo run -p nickel-plane --locked -- check \
  --config "$PLANE_DEMO/instance/main.ncl" --resources "$PLANE_RESOURCES" \
  --targets "$PLANE_DEMO/targets.json" \
  --tf-resources "$TF_RESOURCES" --schema "local=$PLANE_DEMO/schema"
cargo run -p nickel-plane --locked -- render \
  --config "$PLANE_DEMO/instance/main.ncl" --resources "$PLANE_RESOURCES" \
  --targets "$PLANE_DEMO/targets.json" \
  --tf-resources "$TF_RESOURCES" --schema "local=$PLANE_DEMO/schema"
```

The result is a rendered unit template. No infrastructure is applied and no
controller store is created. The instance keeps its default observation policy.
The schema and catalog are outside `instance/`, whose contents become the
confined project module tree.

For execution, targets select exact binary paths; there is no ambient PATH
lookup. External providers require separately prepared schemas and native locks.
The flake's `opentofu-proxmox` package includes bpg/proxmox 0.118.0; a Nix provider
mirror supports native initialization without registry downloads.

The same input flags are used by `render`, `lock`, `plan`, `apply`, `reconcile`
and `serve`. `--k8s-resources` adds the Kubernetes compiler. Repeat
`--schema NAME=/absolute/bundle` for additional frozen provider selections.

| Command | Behavior |
|---|---|
| `check` | Confined offline compilation and domain validation |
| `render` | JSON unit templates and unresolved binding declarations |
| `lock` | Digest receipt to stdout; no configuration values |
| `--locked receipt.json` | Require compilation to match that receipt |
| `plan ... --store /private/store` | Observe and prepare changes; no apply |
| `apply ... --store ... --plan OPERATION` | Execute exactly one stored prepared operation |
| `reconcile ... --store ... --once` | One cycle under the instance's policy |
| `serve ... --store ... --interval-seconds 30` | Single-writer service with resynchronization |
| `status --store ... --instance ID` | Read public status while the service runs |
| `remove --store ... --instance ID --targets ...` | Persist removal and retain resources |
| `remove ... --delete` | Delete only when the stored instance policy permits it |

`plan` contacts targets and invokes OpenTofu/providers or Kubernetes dry-run
admission. It is an execution operation, not an offline compiler command.
Kubernetes dry-run webhooks may run. A Kubernetes preview is a set of
preconditioned operations, not an atomic transaction.

A plan returns operation IDs under `units.<id>.change`. Set `policy.mode` to
`"manage"` **before** planning if you intend to apply. An input revision, target,
artifact, native plan or remote precondition change invalidates that plan.
`apply` refuses stale operations; it does not substitute an unreviewed plan.
Removed units also receive saved deletion plans. Planning stops at the first
pending removed unit in reverse dependency order; after applying that operation,
plan again for the next unit. Independent removed units are serialized too.
Applying a live unit's plan never authorizes deletion of another unit.
`reconcile` with `mode="manage"` authorizes automatic convergence in advance.

## Instance and composition model

An instance has `apiVersion="nickel-plane/v1"`, a stable `id`, a composition
`{name,version}`, parameters, policy and a record of named units. Display names
are optional and have no effect on native resource identity. Keep instance and
unit IDs stable; renaming a unit creates a distinct unit and invokes removal policy
for the old one. Changing the instance ID starts a separate instance; explicitly
remove the old instance when appropriate. Explicit adoption and state migration are not implemented.

A unit declares `backend`, `target`, `configuration`, optional `outputs`,
`dependsOn`, and `bindings`; OpenTofu units also select a frozen `schema` name.
Use `nickel-plane/composition.ncl` for constructors, or plain records. Domain
`composition.define Input build` validates inputs and returns a reusable Nickel
function. `composition.compose` combines named components. OpenTofu rejects
colliding block identities even when their values are equal; Kubernetes rejects
duplicate resource identities during domain validation.

Revision receipts cover transitive source modules, parameters, evaluator, flake
locks, trusted libraries, provider schemas/native locks and rendered templates.
Target execution identity is separately bound into every prepared operation.
Credentials are file references in the target catalog, outside public receipts.

Bindings patch an existing RFC 6901 JSON pointer in a unit's configuration.
They are revalidated by the receiving domain after resolution. For example:

```nickel
bindings = [{
  fromUnit = "infrastructure", output = "message",
  to = "/resources/0/data/message", condition = "applied",
}]
```

OpenTofu outputs use `/outputs/name`. Kubernetes outputs use paths such as
`/resources/Service/web/spec/clusterIP`. Declare the output name upstream with
`{pointer="...", sensitive=false}`. Missing outputs wait; cycles, unknown units,
unknown output declarations, absent destinations and overlapping patches fail.
Defaults gate on `ready`; use `applied` explicitly for infrastructure whose
application health cannot be observed. Dependencies require the current revision
and successful current observations; failed, retained or superseded outputs
cannot satisfy them. OpenTofu expressions within one stack remain symbolic.

Sensitive outputs are omitted from public values and represented by protected
references. Plaintext cross-unit transfer of sensitive outputs is rejected;
a secret-store integration is a future extension. Mark Kubernetes output
selections sensitive when appropriate. Raw credentials must never be embedded
in instance parameters, configuration or ordinary outputs.

## Policy, ownership and recovery

Defaults: `mode="observe"`, `deletion="retain"`, `allowDelete=false`,
`allowReplace=false`. Manage mode permits create/update; destructive plans still
need the corresponding explicit flag. `deletion="delete"` also requires manage
mode and `allowDelete=true`. Removing a unit follows this removal policy.
Kubernetes object pruning inside a surviving unit requires `allowDelete`.
A removed instance remains tombstoned even if the service sees its source again;
there is no automatic resurrection command. A new instance ID is a new identity.

The private 0700 store contains SQLite metadata/journal and per-unit native work
folders. One process holds its writer lease; a second writer fails. Read-only
status remains available. This is not a distributed controller. Keep the store
on a local filesystem supporting flock and SQLite locking; never share copied
stores between active controllers. Target stack bindings are unique in a store.

Operation intent is durable before external mutation. Restart observes native
state and recorded Kubernetes UIDs before preparing new work. Successful and
recovered inventory is durable before its operation is marked finished. Partial
success is recoverable; composition execution is not atomic. Deleted dependents
must finish before upstream resources are removed. Default retention performs
no delete, and completed removal is not replayed.

Status separates Rendered, Applied, Ready, Drifted and Failed. Automatic retries
use exponential delays and stop after five failures for a revision. Explicit
`--retry-failed` resets this budget once; new revisions reset it too. SIGINT and
SIGTERM stop at safe boundaries after the current bounded operation finishes.
Never force-unlock unknown OpenTofu writers. Investigate stale native locks and
ownership conflicts before retrying.

Stop the controller before backup. Copy the **entire private store**, including
native states, operation plans and SQLite files, using protected storage. Restore
it as a unit with original permissions and no competing writer. Native remote
state needs its own consistent backup/versioning. Restoring only SQLite or only
native state can lose ownership information; do not repair that by auto-adoption.
Journal and saved plans are retained for diagnosis; monitor store size. Online
compaction/garbage collection and migration between state locations are deferred.

## Backends

Kubernetes targets require explicit HTTPS server, namespace and optional CA/
private bearer-token file. Credentials are reread per request. Redirects, ambient
proxies and credential exec plugins are disabled. HTTP requires an explicit
loopback-fixture opt-in. Initial execution covers the supported namespaced kinds;
cluster-scoped Namespace creation is deliberately unsupported. Rendering remains
available independently.

New objects use POST; existing owned objects use server-side apply without
force, bound to UID and resourceVersion. This avoids accidental adoption on
create. Ownership labels alone do not authorize adopting an existing object.
Pruning uses recorded inventory plus UID/resourceVersion delete preconditions.
Known workload kinds use observed generation and replica status; other supported
kinds have explicit existence/allocation checks. These checks are not a health
probe of the application. Deletion is asynchronous when Kubernetes finalizers
remain. Array observation is conservative about ordering and length.

A bounded watch on one known object per unit supplies scheduling hints. Expired
watches or transport failures trigger observation; periodic full resynchronization
covers all remaining objects and missed events. The fixture does not certify
real API admission, managedFields/defaulting behavior or a particular CNI.

OpenTofu targets require an absolute pinned binary, stable stack name and optional
private JSON environment file (`{"PROVIDER_TOKEN":"..."}`). Ambient environment
and CLI configuration are cleared. Native state/plans may contain secrets and
remain in private work folders; public diagnostics omit provider response bodies.
Provider-specific environment names, `TF_VAR_*` and HTTP backend credentials are
supported; process configuration/CLI override variables are rejected.

Local state is the default. Optional controller-owned `backendConfig` supports
HTTP with explicit lock/unlock addresses and S3 with `use_lockfile=true`.
Credentials belong in the private environment file. Backends must actually
support their native locking protocol; remote backends are not live-qualified.
Generated compiler artifacts cannot supply their own backend or module blocks.
A reserved built-in `terraform_data.nickel_plane_ownership` resource binds native
state to the controller/store/unit; existing foreign state is refused. It is
created before managed resources and removed after them. A saved plan binds the
exact engine, native provider lock, desired artifact, state and target. Generic
OpenTofu application readiness is reported as unsupported, even after apply.

Upgrade evaluator and engine selections explicitly and regenerate input receipts.
For a **provider lock upgrade in an existing workspace**, reconciliation deliberately
refuses to replace the native lock. Use this maintenance procedure:

1. Stop the controller and back up its entire private store and native state.
2. Acquire the new provider/schema/native lock in a separate disposable directory;
   import a new frozen bundle. Update the instance schema selection and, if needed,
   the pinned OpenTofu binary/provider mirror. Run offline `check` and review the
   new `lock` receipt. Keep target, stack, instance and unit identities unchanged.
3. Locate the existing unit workspace. Its directory is
   `STORE/work-SHA256(INSTANCE_ID + "/" + UNIT_ID)` (UTF-8, no newline). For example:

   ```sh
   unit_key="$(printf '%s' "$instance_id/$unit_id" | sha256sum | cut -d ' ' -f 1)"
   unit_work="$store/work-$unit_key"
   # With the controller stopped and the private workspace verified:
   install -m 600 /absolute/new-bundle/.terraform.lock.hcl "$unit_work/.terraform.lock.hcl"
   ```

4. Preserve `.terraform`, the native state, controller database and journal. Run
   `plan` using the updated source/bundle. Native init selects the new locked
   provider with `-lockfile=readonly`; old plans are invalid. Review replacement
   and deletion effects, then `apply --plan OPERATION` or resume the service under
   an explicitly selected management policy. Regenerate any `--locked` receipt.
5. If planning fails before apply, restore the previous source, bundle, binary and
   native lock together. After apply, provider state migrations may prevent a
   simple downgrade: follow the provider's recovery procedure and preserved backup.

A local Random-provider fixture verifies this lock-maintenance path preserves
resource identity and invalidates old plans; it does not qualify arbitrary
provider version upgrades. Do not change target/backend identity through this
procedure. Adoption, state relocation and general state migration require a
separate explicit integration and are unsupported here.

## Examples and qualification

- `examples/local`: built-in OpenTofu state and outputs, no network provider.
- `examples/kubernetes`: k3s application manifests; existing namespace required.
- `examples/mixed`: OpenTofu output binding into a Kubernetes ConfigMap.
- `examples/proxmox`: stopped VM, disk and network configuration using the pinned
  schema in `schemas/proxmox`. Replace host, node, datastore and bridge explicitly;
  supply `PROXMOX_VE_API_TOKEN` in a private file. The example creates no bootable OS
  image and installs no Kubernetes. VM protection is enabled; deletion would also
  need protection removed through an explicit reviewed update.
- `deploy`: systemd, container build template and Kubernetes deployment template.
  Provision the service account/RBAC, secret/config volumes and private persistent
  store separately. Use one replica with Recreate strategy. Supply a pinned image
  containing Nix and the locked closures; the runtime must support unprivileged
  bubblewrap namespaces. These deployment templates have not been deployed here.
- [`containers`](containers/README.md): Nix-built systemd-nspawn
  rootfs, mixed or OpenTofu-only, with explicit state/configuration mounts,
  private offline Nix state and a disposable VM qualification harness.
  See the [container operation guide](containers/GUIDE.md).
  A [rootless Podman/Quadlet option](containers/podman.md) packages the same
  rootfs with image-bound user units and a separate runtime qualification harness.

Run the [workspace checks](../../docs/development.md#required-checks) first.
For the additional local controller scenario, after preparing the native tools
from that guide, run from the workspace root:

```sh
cargo build -p nickel-plane -p nickel-tf --locked
NICKELING_ROOT=$(pwd -P)
PLANE_BIN="$NICKELING_ROOT/target/debug/nickel-plane" \
  TF_BIN="$NICKELING_ROOT/target/debug/nickel-tf" \
  python3 crates/nickel-plane/tests/local_e2e.py
```

If `CARGO_TARGET_DIR` is set, use its binary paths instead of `target/debug`.

See the [source provenance](../../docs/extraction.md) for provenance. Run the current checks in the development guide; historical qualification reports remain outside this distribution.
