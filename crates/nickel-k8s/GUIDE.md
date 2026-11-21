# nickel-k8s

Compose Kubernetes application manifests in Nickel, validate them against pinned
upstream contracts, and render or lock them with a small Rust CLI. The k3s
profile lives in this package and shares the same application modules.

This first version operates on public configuration for an existing cluster.
It does not install Kubernetes, apply resources, manage nodes or upgrade hosts.

## Build and use

Run from the workspace root on x86_64 Linux, with Nix, Bubblewrap, `prlimit`
and unprivileged user namespaces. Prepare the pinned evaluator explicitly;
`check`, `render` and `lock` do not acquire dependencies.

```sh
NICKELING_ROOT=$(pwd -P)
nix --extra-experimental-features 'nix-command flakes' build \
  path:./crates/nickel-k8s#nickel \
  --no-link --no-update-lock-file --no-write-lock-file
K8S_RESOURCES="$NICKELING_ROOT/crates/nickel-k8s"
K8S_OUTPUT=$(mktemp -d)

cargo run -p nickel-k8s --locked -- check \
  --config "$K8S_RESOURCES/examples/http-k3s.ncl" --resources "$K8S_RESOURCES"
cargo run -p nickel-k8s --locked -- lock \
  --config "$K8S_RESOURCES/examples/http-k3s.ncl" --resources "$K8S_RESOURCES" \
  --output "$K8S_OUTPUT/app.lock.json"
cargo run -p nickel-k8s --locked -- render \
  --config "$K8S_RESOURCES/examples/http-k3s.ncl" --resources "$K8S_RESOURCES" \
  --lock "$K8S_OUTPUT/app.lock.json" --output "$K8S_OUTPUT/app.json"
```

`render` emits a deterministic Kubernetes `List` in JSON, accepted by Kubernetes
tools and also valid YAML. Omit `--output` for stdout. Existing files require
explicit `--replace`; symlink destinations and symlink parents are refused.
Rendering without `--lock` validates current inputs; rendering with it rejects
configuration, imported module, contract, recipe or evaluator changes.

Rust callers use `nickel_k8s::{check, render, lock, Request}`. Set
`default-features = false` to use the library without Clap.

## Composition and profiles

`examples/configmap.ncl` shows ordinary resources;
`examples/composed-configmap.ncl` wraps one in a reusable function with checked
inputs. `examples/http-k3s.ncl`
uses `nickel/application.ncl` to compose a Deployment, Service, ConfigMap,
NetworkPolicy, optional Ingress and optional PersistentVolumeClaim. You can
append other supported raw resources to its returned array.

```nickel
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let application = import "/libraries/nickel-k8s/application.ncl" in
let platform = profiles.k3s { networkPolicy = true } in
{
  target = platform,
  resources = application platform {
    name = "api", namespace = "demo",
    image = "registry.example.com/api:reviewed-version", port = 8080,
    secrets = ["api-credentials"],
  },
}
```

The application module emits an initially denying NetworkPolicy; declare
`network.ingress` and `network.egress` explicitly to allow traffic. The HTTP
example permits the specified Traefik pods to reach port 8080 and permits DNS
to the specified CoreDNS pods. Check selectors against your actual cluster.
NetworkPolicy enforcement requires a functioning supporting controller/CNI.

Both `profiles.kubernetes { ... }` and `profiles.k3s { ... }` take explicit
capabilities: `ingressClass`, `storageClass`, and `networkPolicy`. Defaults are
null/null/false. Choosing k3s does not assume its optional packaged components
are enabled. Ingress and PVC resources must use the declared class, including
PVCs created by StatefulSet and ephemeral-volume claim templates.

Create the namespace and provision referenced Secrets independently. Examples
use documentation image references; select an approved image before deployment.
Secret resources are rejected; keep credential values out of Nickel, ConfigMaps,
locks and these public resource directories. `secrets` and `tlsSecret` are names
of existing Secrets. The local-path example does not provide replicated storage.

## Supported versions and resources

| Profile | Kubernetes version | Generated schema baseline |
|---|---|---|
| kubernetes | 1.34.0 | 1.34.0 |
| k3s v1.34.1+k3s1 | 1.34.1 | 1.34.0 |

These are reproducible compatibility baselines, not recommendations to install
old releases. Unsupported combinations fail; no latest-version resolution or
implicit fallback occurs. The selected nine JSON schemas are byte-identical
between 1.34.0 and 1.34.1 at the pinned upstream source revisions, recorded in
`nickel/contracts/upstream/schema-equivalence.json`. There is no k3s 1.34.0
release. The profile records `kubernetesVersion` and `schemaVersion` separately.

The contracts are unmodified files from
[nickel-kubernetes](https://github.com/nickel-lang/nickel-kubernetes/tree/f3a310db82a408d7e1935b98f7ce103263df5035),
including its required `.js2n-lib`. Revision, per-file SHA-256 and license are
retained in `nickel/contracts/upstream/`. Extending versions or kinds requires
an explicit reviewed bundle update.

Supported kinds: Namespace, ConfigMap, Service, ServiceAccount,
PersistentVolumeClaim, Deployment, StatefulSet, Ingress and NetworkPolicy.
Unknown kinds, CRDs (including HelmChart/HelmChartConfig), malformed identities,
duplicate resource identities and missing namespaces are rejected. Bundles are
limited to 256 resources. Additional Kubernetes admission constraints and
controller-specific behavior remain the API server's responsibility.

## Evaluation and verification

The CLI stages selected public resources before Nix evaluates its pinned
Nickel package. Project modules and named library modules are copied into
separate bounded, read-only sandbox mounts. Only staged modules are hashed;
network and ambient environment access are removed. Absolute `/libraries/...`
imports select the supplied library unambiguously. The resource directory is
explicitly trusted Nix/Nickel code, not a source of untrusted downloads.

Run the workspace checks and native tool preparation from the
[development guide](../../docs/development.md#required-checks).

Tests use the actual pinned evaluator and cover invalid resource fields,
capability mismatches, duplicate identities, transitive lock drift, stable
ordering and file publication. Offline validation does not establish that pods
run, ingress routes traffic, Secrets exist or storage is provisioned. Use an
explicit `kubectl apply --dry-run=server` against the intended cluster to check
its admission rules before a separate deployment decision.

See the [source provenance](../../docs/extraction.md).

## Reusable compositions

`nickel/composition.ncl` provides `define Input build` for typed Nickel functions
and `compose target {componentName = [resources...], ...}` to assemble named
components. The normal domain validation still checks every resource and rejects
duplicate Kubernetes identities. Choose `profiles.k3s` or `profiles.kubernetes`
explicitly; composition does not introduce a separate k3s resource model.

```nickel
let c = import "/libraries/nickel-k8s/composition.ncl" in
let profiles = import "/libraries/nickel-k8s/profiles.ncl" in
let settings = c.define {name | String} (fun p => [{
  apiVersion="v1", kind="ConfigMap",
  metadata={name=p.name, namespace="demo"}, data={mode="production"},
}]) in
c.compose (profiles.kubernetes {}) {settings=settings {name="web"}}
```

Rust callers can use `check_value` for already evaluated JSON through the same
contracts and confined evaluator. Applying, ownership inventory, readiness and
bindings across units belong to [nickel-plane](../nickel-plane/README.md). This
package's existing compiler commands remain offline.
