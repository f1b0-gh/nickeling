{ pkgs, oci, tfOci }:
let
  userspace = import ./userspace.nix { inherit pkgs; };
  extra = [ pkgs.systemd pkgs.util-linux pkgs.python3 pkgs.podman pkgs.dbus pkgs.shadow ];
  closure = userspace.closure extra;
in pkgs.runCommand "nickel-podman-qualification-host" {
  __structuredAttrs = true;
  unsafeDiscardReferences.out = true;
  outputChecks.out.allowedReferences = [];
} ''
  cp -a ${import ./qualification-base.nix { inherit pkgs; }} "$out"
  chmod -R u+w "$out"
  ${userspace.copyClosure closure}
  for package in ${pkgs.lib.escapeShellArgs extra}; do
    for directory in bin sbin; do
      if [ -d "$package/$directory" ]; then
        for binary in "$package/$directory/"*; do
          [ -e "$binary" ] || continue
          ln -sf "$binary" "$out/usr/bin/$(basename "$binary")"
        done
      fi
    done
  done
  mkdir -p "$out/opt/qualification" "$out/usr/lib/systemd/user" "$out/etc/systemd/user-generators"
  cp -a ${pkgs.systemd}/example/systemd/user/. "$out/usr/lib/systemd/user/"
  chmod -R u+w "$out/usr/lib/systemd/user"
  cp ${pkgs.dbus}/etc/systemd/user/dbus.{socket,service} "$out/usr/lib/systemd/user/"
  substituteInPlace "$out/usr/lib/systemd/user/dbus.service" \
    --replace-fail '--session --address' '--config-file=${pkgs.dbus}/share/dbus-1/session.conf --address'
  mkdir -p "$out/usr/lib/systemd/user/sockets.target.wants"
  ln -s ../dbus.socket "$out/usr/lib/systemd/user/sockets.target.wants/dbus.socket"
  ln -s ${pkgs.podman}/libexec/podman/quadlet "$out/etc/systemd/user-generators/podman-user-generator"
  cp ${oci.image} "$out/opt/qualification/image.tar.gz"
  cp ${tfOci.image} "$out/opt/qualification/image-tf.tar.gz"
  cp -r ${oci.quadlet} "$out/opt/qualification/quadlet"
  cp -r ${tfOci.quadlet} "$out/opt/qualification/quadlet-tf"
  cp ${../../tests/local_e2e.py} "$out/opt/qualification/local_e2e.py"
  cp ${../../examples/mixed/main.ncl} "$out/opt/qualification/main.ncl"
  ln -s ${pkgs.shadow} "$out/opt/qualification/shadow"
  printf 'operator:x:1000:1000:Qualification operator:/home/operator:/bin/sh\n' >> "$out/etc/passwd"
  printf 'operator:x:1000:\n' >> "$out/etc/group"
  printf 'operator:!:::::::\n' >> "$out/etc/shadow"
  printf 'operator:65536:65536\n' > "$out/etc/subuid"
  printf 'operator:65536:65536\n' > "$out/etc/subgid"
  # These units and setuid helpers are used only inside the disposable VM.
  cat > "$out/etc/systemd/system/qualification-user.service" <<'EOF'
  [Service]
  User=operator
  Group=operator
  Environment=HOME=/home/operator XDG_RUNTIME_DIR=/run/user/1000 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
  ExecStart=${pkgs.systemd}/lib/systemd/systemd --user
  Delegate=yes
  EOF
''
