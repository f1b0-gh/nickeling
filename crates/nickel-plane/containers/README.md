# nickel-plane containers

Nix recipes for running nickel-plane as a service container.

These recipes build a self-contained rootfs with nickel-plane, systemd, the
selected engines and their resources. Two variants are available: Kubernetes
and OpenTofu together, or OpenTofu alone. No sibling checkout is required.

| Runtime | Documentation | Outputs |
| --- | --- | --- |
| systemd-nspawn | [Operations guide](GUIDE.md) | `container-rootfs`, `container-rootfs-tf` |
| Rootless Podman / Quadlet | [Podman guide](podman.md) | `oci-image`, `oci-image-tf`, `quadlet`, `quadlet-tf` |

## Build a rootfs

From the Nickeling root, with Nix and its `nix-command flakes` features:

```sh
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers container-rootfs \
  --out-link result-container --max-jobs 1 --cores 2
```

Building does not start a container or install a host service. The guides cover
namespace prerequisites, volumes, credentials, network restrictions, private
Nix initialization and updates.

## State and resources

The container runs a controller as a service, without a kernel or boot image. Instance data and credentials are supplied at deployment, outside the
image. External providers must be prepared explicitly with their schemas and locks.

Layout checks, VM tests and validation in a real environment are separate steps.
The drivers in `tests/` are retained; generated disks, images, logs and test
results stay outside the repository.
