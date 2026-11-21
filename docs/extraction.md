# Source provenance and distribution boundaries

Nickeling contains four Rust components: the shared `nickel-util` foundation,
the independent `nickel-k8s` and `nickel-tf` compilers, and the `nickel-plane`
controller. Their sources originated in Erwin and Organistalt. Those names are
historical; no checkout of either project is required by this source layout.

## Distributed sources

The distribution retains maintained source, tests, examples, Nickel/Nix
resources, third-party contracts and notices, manifests, lockfiles and user
documentation. The initial extraction included unique local changes and the
controller's container recipes, preserving versions and locks.

It starts an independent history. Previous commits, runtime state, caches,
raw reports and historical qualification evidence are outside this distribution.
The initial selection inventory and fingerprints remain in the original local
repository; subsequent commits identify distributed revisions. A crate version
alone does not identify an unpublished source snapshot.

## Retained originals

The original repositories retain their history, extraction inventories, reports,
irreplaceable inputs and unique experimental source. Preparing this distribution
does not delete or rewrite them. Historical bootc experiments are not Nickeling
components. The controller container recipes build a service, not a bootable OS.

Historical evidence does not qualify the current sources. Run the current
[development checks](development.md#required-checks); local fixtures do not
qualify a live cluster, provider deployment or remote CI workflow.
