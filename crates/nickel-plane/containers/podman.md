# Rootless Podman and Quadlet

This option packages the Nix rootfs in a Docker-format image archive loadable by
Podman. The outputs are named `oci-image` and `oci-image-tf`; they are not
`oci-archive` files. The container retains systemd, private Nix initialization
and the Rust `nickel-plane` service. Qualification drivers are retained in
`tests/`. Historical reports remain in the original local repository and do not
qualify a new build.

## Build

From the Nickeling repository root:

```sh
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers oci-image --out-link result-podman-image --max-jobs 1 --cores 2
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers quadlet --out-link result-podman-quadlet --max-jobs 1 --cores 2
nix --extra-experimental-features 'nix-command flakes' build --impure \
  --file crates/nickel-plane/containers checks.oci checks.tf-oci --no-link --max-jobs 1 --cores 2
```

For OpenTofu alone, select `oci-image-tf` and `quadlet-tf` together. This omits
Kubernetes resources. Packages and locks are the same as for nspawn. The
Containerfile template under `crates/nickel-plane/deploy/` is not used.

The archive contains a self-contained Nix closure. The Quadlet is bound to its
`sha256:…` image ID, also recorded in `result-podman-quadlet/image-id`.
It uses `Pull=never`: startup consults neither a registry nor a mutable tag.
Explicitly load the archive on the deployment host as the account that will
run the service:

```sh
podman load --input result-podman-image
podman image inspect "$(cat result-podman-quadlet/image-id)"
```

## Prepare the account and volumes

Current images target x86_64 Linux. The host needs Podman/Quadlet, cgroup v2,
a systemd user manager, `/etc/subuid` and `/etc/subgid` ranges, and
`newuidmap`/`newgidmap`. These helpers must be in the user services' `PATH`.
Nested namespaces, in-container OverlayFS and host security policy must support
the confinement described below. Account preparation and optional linger setup
are explicit administrator operations; building the image does not perform them.

The user account and its primary group map to `1000:1000` inside the container
through `keep-id:uid=1000,gid=1000`. Bind mounts preserve ownership; no `:U`
suffix is used.

For a new installation, create a **previously nonexistent** private store at
`~/.local/share/nickel-plane`, owned by the selected account with mode `0700`.
If a store already exists there, preserve its identity and explicitly decide
whether to reuse it. This option neither migrates nor adopts an nspawn store.

Prepare configuration under `~/.config/nickel-plane`:

```text
container.env
controller.conf
targets.json
instance/main.ncl
schemas/local/...
credentials/...
```

`container.env` holds a deployment-specific UUID, generated once:

```ini
container_uuid=REPLACE_WITH_A_UNIQUE_UUID
```

Keep this value across restarts and image changes. `controller.conf` uses the
arguments from the [nspawn deployment](GUIDE.md), for example:

```ini
PLANE_ARGS="--tf-resources /opt/nickel/tf --k8s-resources /opt/nickel/k8s --schema local=/etc/nickel-plane/schemas/local"
```

Omit `--k8s-resources` for the TF variant. Target credentials remain in private
files with mode `0600`, referenced by `targets.json`. The configuration directory
is mounted read-only. Keep secrets out of `.env` files and arguments; the
controller reads explicitly selected credential files.

## Install both service parts

The `quadlet` output contains a `.container` file **and** a systemd override.
Install both under the same service name:

```sh
mkdir -p ~/.config/containers/systemd
mkdir -p ~/.config/systemd/user/nickel-plane.service.d
install -m 0644 result-podman-quadlet/nickel-plane.service.d/10-container-id.conf \
  ~/.config/systemd/user/nickel-plane.service.d/10-container-id.conf
install -m 0644 result-podman-quadlet/nickel-plane.container \
  ~/.config/containers/systemd/nickel-plane.container
systemctl --user daemon-reload
systemctl --user start nickel-plane.service
```

These are operator actions on the selected host. For a new installation, verify
that the file and service names are unused. For another instance, change the
`.container` name, `.service.d` directory, `ContainerName`, both volume paths
and the UUID.

The override replaces Quadlet's generated name-based cleanup. Each launch writes
its own CID file in the service's temporary directory, named with `INVOCATION_ID`;
stop and cleanup target only that ID. A name collision must preserve the existing
container. Without the override, a systemd condition prevents startup. The
`podman` executable must be available in `/usr/bin` or `/bin`, the paths declared
by the override for stop commands. For a custom installation path, adjust
`ExecSearchPath` in an additional override.

Quadlet's `[Install] WantedBy=default.target` requests startup with the user
manager. Generated units are not enabled through an ordinary `systemctl enable`.
Startup without an open session depends on the account's linger configuration,
which is not installed automatically.

## Confinement and networking

The container starts as root **inside its user namespace**. `SYS_ADMIN` in that
namespace supports systemd and initialization of Nix OverlayFS and private procfs.
The `nickel-plane` service then drops to `1000:1000` with an empty capability set.
It retains `NoNewPrivileges`, file protections and inner Bubblewrap confinement.

The Quadlet does not request `--privileged`, disable seccomp/AppArmor or remove
`/proc` masks. No host Docker/Podman socket or Nix daemon is mounted. If required
namespaces or mounts are denied, execution must fail.

The image root is read-only; `/var`, `/tmp` and `/run` are bounded tmpfs mounts.
Only the declared store persists. Private Nix state is rebuilt at each startup
and is not a controller backup.

`Network=none` provides loopback only; host proxies and hosts entries are not
inherited. For real targets, the operator must configure a dedicated network,
DNS and outbound access rules. No inbound port is required. Acquiring external
providers remains an explicit preparation step, separate from `check/render/lock`.

## Updates and backup

Stop the service, back up the entire store, build and load the new archive,
replace Quadlet and override together, then reload systemd and start the service.
Preserve the store, ownership, targets and UUID. Image tags do not trigger
automatic updates. The [nspawn guide's backup and state compatibility rules](GUIDE.md#updates-state-and-rollback)
also apply. Recreating a container does not undo infrastructure mutations.

## Reproduce local qualification

Build `qualification-podman` and `qualification-tools.{qemu,mkfs}` through the
same Nix entry point, then run:

```sh
python3 crates/nickel-plane/containers/tests/qualify.py --runtime podman \
  --rootfs /path/to/qualification-podman \
  --qemu /path/to/qemu-system-x86_64 --mkfs /path/to/mkfs.ext4 \
  --output /path/to/a-new-directory
```

The harness requires access to `/dev/kvm` and boots a disposable VM without a
network interface. It prepares the account, bus and systemd user manager inside
that VM, using Podman VFS storage. Commands, test sources, logs and results are
recorded in the selected directory. The disposable disk can be removed after
QEMU stops; retain the other receipts. The harness configures no services on the
development host.

References: [official Quadlet documentation](https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html),
[Podman options](https://docs.podman.io/en/latest/markdown/podman-run.1.html).
