# Test-only VM host, adapted from the historical minimal-bootc image builder.
# No bootc, network configuration or OS lifecycle is included or qualified.
{ pkgs }:
let
  inherit (pkgs) lib;
  userspace = import ./userspace.nix { inherit pkgs; };
  # Like the historical test host, start only the minimal system manager.
  # The full systemd package (including nspawn) is added by qualification.nix;
  # its login/session services need D-Bus, which this fixture does not provide.
  systemd = pkgs.systemdMinimal.override {
    withNetworkd = false; withResolved = false; withTimesyncd = false;
    withSysusers = true; withAcl = true; withAudit = true; withLibseccomp = true;
  };
  kernel = pkgs.linuxPackages_6_12.kernel;
  moduleNames = [ "virtio_pci" "virtio_scsi" "sd_mod" "virtio_console" "virtio_rng" "ext4" "overlay" "autofs4" ];
  modules = pkgs.makeModulesClosure {
    kernel = lib.getOutput "modules" kernel;
    rootModules = moduleNames;
    firmware = pkgs.runCommand "qualification-firmware" {} "mkdir -p $out/lib/firmware";
    allowMissing = false;
  };
  selected = [ pkgs.busybox systemd pkgs.kmod pkgs.qemu_kvm.ga ];
  closure = userspace.closure selected;
  init = pkgs.writeScript "nickel-qualification-init" ''
    #!${pkgs.busybox}/bin/sh
    export PATH=${pkgs.busybox}/bin:${pkgs.kmod}/bin
    mkdir -p /proc /sys /dev /run /sysroot
    mount -t proc proc /proc
    mount -t sysfs sysfs /sys
    mount -t devtmpfs devtmpfs /dev
    mount -t tmpfs tmpfs /run
    fail() { echo "QUALIFICATION_INIT_FAILED: $*" >/dev/console; exec sh; }
    for module in ${lib.escapeShellArgs moduleNames}; do
      ${pkgs.kmod}/bin/modprobe -d /modules "$module" || fail "module $module"
    done
    for attempt in $(seq 1 100); do
      [ -b /dev/sda ] && break
      sleep 0.1
    done
    mount -o rw /dev/sda /sysroot || fail "mount root"
    mkdir -p /sysroot/proc /sysroot/sys /sysroot/dev /sysroot/run
    ln -s /usr/lib/qualification /run/current-system
    mount --move /proc /sysroot/proc
    mount --move /sys /sysroot/sys
    mount --move /dev /sysroot/dev
    mount --move /run /sysroot/run
    exec switch_root /sysroot /usr/lib/systemd/systemd
  '';
  initrd = pkgs.makeInitrdNG {
    name = "nickel-qualification-initrd";
    contents = [
      { source = init; target = "/init"; }
      { source = pkgs.busybox + "/bin"; }
      { source = pkgs.kmod + "/bin"; }
      { source = modules; target = "/modules"; }
    ];
    compressor = "zstd";
  };
in pkgs.runCommand "nickel-container-qualification-base" {
  __structuredAttrs = true;
  unsafeDiscardReferences.out = true;
  outputChecks.out.allowedReferences = [];
} ''
  mkdir -p "$out"/{dev,proc,sys,run,tmp,var,etc,usr/bin,usr/lib/nix/store,usr/lib/qualification,usr/lib/modules/${kernel.modDirVersion}}
  ln -s usr/lib/nix "$out/nix"
  ln -s usr/bin "$out/bin"
  ln -s usr/bin "$out/sbin"
  ln -s usr/lib "$out/lib"
  ${userspace.copyClosure closure}
  for package in ${lib.escapeShellArgs selected}; do
    for directory in bin sbin; do
      if [ -d "$package/$directory" ]; then
        for binary in "$package/$directory/"*; do
          [ -e "$binary" ] || continue
          ln -sf "$binary" "$out/usr/bin/$(basename "$binary")"
        done
      fi
    done
  done
  cp -a ${systemd}/lib/systemd "$out/usr/lib/systemd"
  cp -a ${systemd}/lib/udev "$out/usr/lib/udev"
  chmod -R u+w "$out/usr/lib/systemd"
  cp -a ${systemd}/example/systemd/system "$out/usr/lib/systemd/system"
  chmod -R u+w "$out/usr/lib/systemd"
  ln -s ${systemd} "$out/usr/lib/qualification/systemd"
  ln -s /usr "$out/usr/lib/qualification/sw"
  mkdir -p "$out/usr/lib/systemd/system/service.d" "$out/usr/lib/tmpfiles.d" "$out/usr/lib/sysusers.d"
  cp -a ${systemd}/example/tmpfiles.d/. "$out/usr/lib/tmpfiles.d/"
  cp -a ${systemd}/example/sysusers.d/. "$out/usr/lib/sysusers.d/"
  chmod -R u+w "$out/usr/lib/tmpfiles.d" "$out/usr/lib/sysusers.d"
  cat > "$out/usr/lib/systemd/system/service.d/10-command-path.conf" <<'EOF'
  [Service]
  Environment=PATH=/usr/bin
  ExecSearchPath=/usr/bin
  EOF
  cp ${kernel}/bzImage "$out/usr/lib/modules/${kernel.modDirVersion}/vmlinuz"
  cp ${initrd}/initrd "$out/usr/lib/modules/${kernel.modDirVersion}/initramfs.img"
  cp -aL ${modules}/lib/modules/${kernel.modDirVersion}/. "$out/usr/lib/modules/${kernel.modDirVersion}/"
  printf 'ID=nickel-qualification\nNAME="Disposable nspawn qualification host"\n' > "$out/usr/lib/os-release"
  ln -s ../usr/lib/os-release "$out/etc/os-release"
  printf 'root:x:0:0:root:/root:/bin/sh\n' > "$out/etc/passwd"
  printf 'root:x:0:\nsystemd-journal:x:190:\n' > "$out/etc/group"
  printf 'root:!:::::::\n' > "$out/etc/shadow"
  chmod 0600 "$out/etc/shadow"
  printf 'u root 0 "Root" /root /bin/sh\n' > "$out/usr/lib/sysusers.d/root.conf"
  printf 'd /var/log 0755 root root -\nd /var/lib 0755 root root -\nL /var/run - - - - ../run\n' > "$out/usr/lib/tmpfiles.d/state.conf"
  printf 'hosts: files\npasswd: files\ngroup: files\n' > "$out/etc/nsswitch.conf"
  touch "$out/etc/machine-id"
  mkdir -p "$out/etc/systemd/system/multi-user.target.wants" "$out/etc/udev/rules.d"
  ln -s /usr/lib/systemd/system/multi-user.target "$out/etc/systemd/system/default.target"
  cat > "$out/usr/lib/systemd/system/qemu-guest-agent.service" <<'EOF'
  [Unit]
  Description=Disposable qualification guest agent
  ConditionPathExists=/dev/virtio-ports/org.qemu.guest_agent.0
  [Service]
  ExecStart=/usr/bin/qemu-ga --statedir /run/qemu-ga
  RuntimeDirectory=qemu-ga
  Restart=on-failure
  RestartSec=5
  [Install]
  WantedBy=multi-user.target
  EOF
  ln -s /usr/lib/systemd/system/qemu-guest-agent.service "$out/etc/systemd/system/multi-user.target.wants/"
  printf 'SUBSYSTEM=="virtio-ports", ATTR{name}=="org.qemu.guest_agent.0", TAG+="systemd", ENV{SYSTEMD_WANTS}="qemu-guest-agent.service"\n' > "$out/etc/udev/rules.d/80-qemu-agent.rules"
  chmod 1777 "$out/tmp"
''
