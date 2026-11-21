#!/bin/sh
set -eu
export PATH=/usr/bin
export NIX_IGNORE_SYMLINK_STORE=1
export NIX_REMOTE=local
# /var is a container-private tmpfs. Only the controller store is persistent.
test "$(stat -c %u /var/lib/nickel-plane)" = 1000
test "$(stat -c %a /var/lib/nickel-plane)" = 700
mkdir -p /var/lib/nickel-nix/upper /var/lib/nickel-nix/work /var/lib/nickel-nix/nix/db /var/cache/nickel-plane
mount -t overlay overlay -o userxattr,lowerdir=/usr/lib/nix/store,upperdir=/var/lib/nickel-nix/upper,workdir=/var/lib/nickel-nix/work /usr/lib/nix/store
nix-store --load-db < /usr/share/nickel-container/registration
chown 1000:1000 /usr/lib/nix/store
chown -R 1000:1000 /var/lib/nickel-nix/nix /var/cache/nickel-plane
chmod 0700 /var/lib/nickel-nix/nix /var/cache/nickel-plane
# nspawn masks parts of /proc. Linux then refuses nested unprivileged procfs
# mounts unless an unobscured procfs is present in the parent mount namespace.
# Keep the container's original /proc masks and isolate this anchor behind a
# root-only directory. It is the container PID namespace, never the host /proc.
mkdir -p /run/nickel-proc-private/proc
chmod 0700 /run/nickel-proc-private
mount -t proc -o nosuid,nodev,noexec proc /run/nickel-proc-private/proc
