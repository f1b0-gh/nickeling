{ pkgs, rootfs }:
let
  extra = [ pkgs.systemd pkgs.util-linux pkgs.python3 ];
  userspace = import ./userspace.nix { inherit pkgs; };
  closure = userspace.closure extra;
in pkgs.runCommand "nickel-container-qualification-host" {
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
  mkdir -p "$out/var/lib/machines" "$out/opt/qualification" "$out/etc/systemd/nspawn"
  cp -a ${rootfs} "$out/var/lib/machines/plane-v1"
  cp ${../deploy/nickel-plane.nspawn} "$out/etc/systemd/nspawn/plane.nspawn"
  cp ${../tests/guest.py} "$out/opt/qualification/guest.py"
  cp ${../../tests/local_e2e.py} "$out/opt/qualification/local_e2e.py"
  cp ${../../examples/mixed/main.ncl} "$out/opt/qualification/main.ncl"
  # Every configuration below belongs solely to the disposable test VM.
  cat > "$out/etc/systemd/system/qualification-container@.service" <<'EOF'
  [Unit]
  Description=Disposable nspawn test instance %i
  [Service]
  Type=notify
  NotifyAccess=all
  EnvironmentFile=/run/qualification-%i.conf
  ExecStart=/usr/bin/systemd-nspawn --keep-unit --register=no --settings=yes --machine=%i --uuid=''${CONTAINER_UUID} --directory=''${CONTAINER_ROOTFS}
  Delegate=yes
  KillMode=mixed
  TimeoutStopSec=60
  EOF
  # Host networking is not needed: the test VM has no network device.
  rm -f "$out/etc/systemd/system/multi-user.target.wants/systemd-networkd.service" \
    "$out/etc/systemd/system/multi-user.target.wants/systemd-resolved.service" \
    "$out/etc/systemd/system/multi-user.target.wants/systemd-timesyncd.service"
''
