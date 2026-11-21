# Nix-built nickel-plane system container

This target packages nickel-plane for systemd-nspawn. It uses the package
catalog pinned by `../flake.lock` and builds the Rust workspace
in this repository. No sibling checkout is required. It includes no kernel,
initramfs, bootc updater, OSTree or VM guest agent. The
[rootless Podman/Quadlet option](podman.md) packages the same rootfs as a loadable
image with user units. Incus is not implemented by this target.

Historical qualification reports remain in the original local repository.
Run the checks below for the selected source revision; an older result does not
qualify a new build.

## Build

Run from the Nickeling repository root:

```sh
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers container-rootfs --out-link result-container --max-jobs 1 --cores 2
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers checks.layout checks.tf-layout checks.contracts checks.profile \
  --no-link --max-jobs 1 --cores 2
```

`container-rootfs-tf` builds the CLI with only its OpenTofu adapter and omits
Kubernetes resources. The Kubernetes adapter uses its Rust HTTP client and needs
no kubectl. Both variants build only the selected `nickel-plane` CLI, with at most
two Cargo jobs and no implicit default adapter features. They use the existing
locked nixpkgs revision
`1c89083e451e891152365b799595cf7790a43325` and Cargo.lock. Nix snapshots the selected
source files; this is a local source build, not a separately published release.

`profiles/plane.ncl` owns public selection. Its exported JSON is checked for
freshness. A custom JSON value can be passed through Nix's `profile`
argument; the recipe independently rejects unknown fields/adapters, duplicates,
an empty selection and unsupported architecture/version. These container profiles
do not configure a bootable OS.

The rootfs contains a package inventory and Nix registration at
`/usr/share/nickel-container/`. Runtime paths retain `/nix/store` through
`/nix -> usr/lib/nix`. The packaged resource flakes freeze the selected Nickel
output path from the locked build. Their `provenance/` directories retain the
original resource flakes and locks. Runtime evaluation still uses real Nix flake
evaluation and closure queries, but requires neither the Nixpkgs source tree nor
its fetch cache. Changing the evaluator requires rebuilding the image.

## Host and deployment

The host needs systemd-nspawn, cgroup v2 delegation, user/mount/PID/network
namespaces, OverlayFS and a filesystem supporting idmapped mounts (the
qualification host uses ext4). Root launches the outer container; nickel-plane
runs as UID/GID 1000 with no capabilities. Rootless nspawn is not covered.

Review and install [`deploy/nickel-plane.nspawn`](deploy/nickel-plane.nspawn) under
the host's `/etc/systemd/nspawn/`, and the host unit
[`deploy/nickel-plane-container.service`](deploy/nickel-plane-container.service)
under `/etc/systemd/system/`. These commands are **operator actions on the selected
deployment host**; building or testing does not install host services.

The example reserves namespace range `65536:65536`. Select an unused subordinate
UID/GID range on the host; never overlap unrelated containers. The rootfs uses
idmapped mounts, and explicit `:idmap` binds keep state ownership at UID/GID 1000
on both sides. Reserve that host identity for this deployment and create
`/var/lib/nickel-plane` with owner `1000:1000` and mode `0700`. Initialization checks
ownership; it does not recursively take ownership of an arbitrary existing store.

Install the versioned rootfs at a stable host path and create the root-owned file
`/etc/nickel-plane-container.conf`:

```ini
CONTAINER_ROOTFS=/var/lib/machines/nickel-plane-v1
CONTAINER_UUID=REPLACE_WITH_A_UNIQUE_UUID
```

Generate a UUID once per deployment, for example with `uuidgen`; preserve it across
restarts and image replacements. Each separate container needs its own UUID.
The distributed image contains an empty machine-id, never the build host's ID.

Create this explicit host configuration tree:

```text
/etc/nickel-plane/
  controller.conf
  targets.json
  instance/main.ncl
  schemas/local/...
  credentials/...
```

For a mixed deployment, `controller.conf` contains:

```ini
PLANE_ARGS="--tf-resources /opt/nickel/tf --k8s-resources /opt/nickel/k8s --schema local=/etc/nickel-plane/schemas/local"
```

For TF-only, omit `--k8s-resources`. Add explicit schema mappings as required.
Values here are CLI argument tokens, not shell code. Credential values belong in
dedicated files referenced by targets.json, with owner 1000 and mode 0600; the
configuration directory is mounted read-only. No credentials are embedded during
the build. See [nickel-plane target configuration](../GUIDE.md#backends).

After provisioning, the operator can enable/start the host unit:

```sh
systemctl daemon-reload
systemctl enable --now nickel-plane-container.service
```

The packaged controller service starts only when the instance file exists. It
requires successful private Nix initialization, then runs with `NoNewPrivileges`,
no capabilities, read-only system files and declared writable directories.

## Networking and provider preparation

The supplied nspawn configuration creates a private network namespace with only
loopback, no veth, no DNS inheritance and no exposed ports. It supports the local
fixtures and denies external connectivity by construction. It does not inherit
bootc's interface activation, firewall or time-sync services.

For real targets, the operator must provision a dedicated network namespace or
veth route, explicit DNS and host-side destination/port filtering. Permit only the
chosen Kubernetes API or provider endpoints; permit registry DNS/HTTPS only for
the separate provider acquisition step. Do not share the host network namespace
as a shortcut. No inbound port is needed by the controller. Container time comes
from the host; it does not run an NTP client.

The delivered OpenTofu is pinned and supports built-in resources without provider
downloads. External providers require explicit packaging or a verified provider
mirror, matching frozen schemas, exact versions and native `.terraform.lock.hcl`
checksums. Prepare those outside evaluation and include their runtime closure in
the Nix image; do not mount the host's entire Nix store or daemon socket. Provider
installation is not part of `check`, `render` or `lock`.

## Private Nix state and confinement

`/var` and `/tmp` are bounded tmpfs mounts. Only `/var/lib/nickel-plane` persists.
At container startup a root oneshot mounts an OverlayFS over the delivered store,
loads its closure registration into a private Nix database, and assigns the scratch
store/database to UID 1000. The lower store remains in the read-only image. Both
the overlay and database are discarded on stop and reconstructed from the next
image; they are not a controller backup and need not survive an update.

The service uses the local Nix store explicitly, with no substituters or global
flake registry. It evaluates the bundled resource flakes offline. A writable
scratch store is necessary because the current nickel-util path stages flakes
before evaluation. No Nix daemon runs inside the container and no host socket is
bound in.

nspawn masks parts of its `/proc`, which causes Linux to reject Bubblewrap's new
procfs mount. Initialization therefore mounts a second instance of the **container's**
procfs beneath `/run/nickel-proc-private`, owned by root with mode 0700. The service
identity cannot traverse that directory. The mount must remain writable for the
kernel's nested-mount check; a read-only mount was tested and still rejected. The
service's filesystem protection preserves this mount's flags, while its directory
permissions prevent access. The usual nspawn `/proc` masks remain in place. No
host procfs is bound in, no capability is added to the controller, and Bubblewrap
still creates its own PID namespace and procfs. See the
[upstream nspawn issue](https://github.com/systemd/systemd/issues/34226).

Nickel itself still runs through nickel-util's Bubblewrap path: separate user,
mount, PID and network namespaces, only the pinned Nickel closure and read-only
staged modules/libraries, a 1 GiB address-space bound, 20 CPU seconds and a 30-second
process deadline. The outer container does not replace that boundary. A host that
denies those namespace operations must fail the evaluator, not disable confinement.

## Updates, state and rollback

Stop the host unit, back up the whole controller volume, install a separately built
versioned rootfs, change `CONTAINER_ROOTFS`, and start the unit again. Preserve the
UUID, state directory, target identities and UID mapping. Do not use `bootc upgrade`
inside this container.

Take backups with the controller stopped, preserving ownership and mode. Include
the whole store: SQLite journal, controller identity/inventory, native OpenTofu
working directories/state/locks and operation records. Back up configuration and
credentials separately according to their access policy. Never restore an old
snapshot beside a running writer; the same-volume lease is not distributed leader
election and does not protect independently copied volumes.

Replacing an image does not undo infrastructure mutations or state migrations.
This controller accepts store schema version 1 and refuses unknown versions. An
older binary can only be used when compatible with the current store and native
state formats; restoring an older backup requires reconciling real target state
before management resumes. Preserve interrupted-operation records for recovery.

## Local qualification

Build `qualification-host` and `qualification-tools.{qemu,mkfs}` through the same
Nix entry point (also with `--max-jobs 1 --cores 2`), then run `python3 crates/nickel-plane/containers/tests/qualify.py --rootfs HOST --qemu QEMU_BIN --mkfs
MKFS_EXT4_BIN --output NEW_EVIDENCE_DIRECTORY`. The test-only host is built here from the same Nixpkgs lock, with kernel 6.12
and a guest-agent transport included in these tests. It contains no bootc updater.
This boots a disposable ext4 VM with no network device and invokes nspawn there. It needs user access to `/dev/kvm`,
not host sudo. It records the command, serial output, test-source archive, guest
events and qualification log. Disk images are disposable and can be removed after
the VM exits; retain logs/receipts.

The fixture uses real pinned Nickel and OpenTofu plus an HTTP Kubernetes stub. It
does not qualify a live Kubernetes/k3s cluster, Proxmox, external provider network
access or bootc installation/update/rollback. The separate
[Podman operation guide](podman.md) describes the rootless option.
Its historical qualification reports are not included in this source distribution.
