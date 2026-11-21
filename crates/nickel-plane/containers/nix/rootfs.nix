{ pkgs, profile, packages }:
let
  inherit (pkgs) lib;
  userspace = import ./userspace.nix { inherit pkgs; };
  valid = builtins.isAttrs profile
    && builtins.attrNames profile == [ "adapters" "architecture" "revision" "version" ]
    && builtins.isInt profile.version && profile.version == 1
    && builtins.isString profile.architecture && profile.architecture == "x86_64_linux"
    && builtins.isString profile.revision && builtins.match "[0-9]+" profile.revision != null
    && builtins.isList profile.adapters && profile.adapters != []
    && lib.unique profile.adapters == profile.adapters
    && lib.all (name: builtins.isString name && builtins.elem name [ "opentofu" "kubernetes" ]) profile.adapters;
  has = name: builtins.elem name profile.adapters;
  systemd = pkgs.systemdMinimal.override {
    withNetworkd = false; withResolved = false; withTimesyncd = false;
    withSysusers = true; withAcl = true; withAudit = true; withLibseccomp = true;
  };
  selected = (userspace.select [ "busybox" "ca-certificates" ]) ++ [
    systemd packages.plane pkgs.nix pkgs.nickel pkgs.bubblewrap pkgs.util-linux
  ] ++ lib.optional (has "opentofu") pkgs.opentofu;
  resources = [ packages.planeResources ]
    ++ lib.optional (has "opentofu") packages.tfResources
    ++ lib.optional (has "kubernetes") packages.k8sResources;
  closure = userspace.closure (selected ++ resources);
in
assert lib.assertMsg valid "Invalid container profile: version 1, x86_64_linux, numeric revision and unique supported adapters required";
pkgs.runCommand "nickel-plane-container-rootfs-${profile.revision}" {
  __structuredAttrs = true;
  nativeBuildInputs = [ pkgs.python3 ];
  unsafeDiscardReferences.out = true;
  outputChecks.out.allowedReferences = [];
  passthru = { inherit profile packages; };
} ''
  mkdir -p "$out"/{dev,proc,sys,run,tmp,var/lib/nickel-plane,etc/nix,etc/nickel-plane,usr/bin,usr/lib/nix/store,usr/libexec,usr/share/nickel-container,opt/nickel}
  ln -s usr/lib/nix "$out/nix"
  ln -s /var/lib/nickel-nix "$out/usr/lib/nix/var"
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
  ${lib.optionalString (has "opentofu") ''
    # Execution targets reject symlink components. A regular immutable launcher
    # preserves that policy while keeping the compiled store references intact.
    rm "$out/usr/bin/tofu"
    cat > "$out/usr/bin/tofu" <<'EOF'
    #!/usr/bin/sh
    exec ${pkgs.opentofu}/bin/tofu "$@"
    EOF
    chmod 0755 "$out/usr/bin/tofu"
  ''}
  # Resource directories must be real directories: nickel-util rejects symlinks.
  cp -r ${packages.planeResources} "$out/opt/nickel/plane"
  ${lib.optionalString (has "opentofu") ''cp -r ${packages.tfResources} "$out/opt/nickel/tf"''}
  ${lib.optionalString (has "kubernetes") ''cp -r ${packages.k8sResources} "$out/opt/nickel/k8s"''}
  cp -a ${systemd}/lib/systemd "$out/usr/lib/systemd"
  chmod u+w "$out/usr/lib/systemd"
  cp -a ${systemd}/example/systemd/system "$out/usr/lib/systemd/system"
  chmod -R u+w "$out/usr/lib/systemd"
  mkdir -p "$out/usr/lib/systemd/system/service.d" "$out/etc/systemd/system/multi-user.target.wants"
  cat > "$out/usr/lib/systemd/system/service.d/10-path.conf" <<'EOF'
  [Service]
  Environment=PATH=/usr/bin
  ExecSearchPath=/usr/bin
  EOF
  cp ${../deploy/nickel-nix-init.sh} "$out/usr/libexec/nickel-nix-init"
  cp ${../deploy/nickel-nix-init.service} "$out/usr/lib/systemd/system/nickel-nix-init.service"
  cp ${../deploy/nickel-plane.service} "$out/usr/lib/systemd/system/nickel-plane.service"
  ln -s /usr/lib/systemd/system/multi-user.target "$out/etc/systemd/system/default.target"
  ln -s /usr/lib/systemd/system/nickel-plane.service "$out/etc/systemd/system/multi-user.target.wants/nickel-plane.service"
  ln -sf /usr/lib/systemd/systemd "$out/usr/bin/init"
  ln -sf /usr/lib/systemd/systemd "$out/usr/bin/systemd"
  printf 'root:x:0:0:root:/root:/bin/sh\nnickel-plane:x:1000:1000:Nickel plane:/var/cache/nickel-plane:/bin/false\n' > "$out/etc/passwd"
  printf 'root:x:0:\nsystemd-journal:x:190:\nnickel-plane:x:1000:\n' > "$out/etc/group"
  printf 'root:!:::::::\nnickel-plane:!:::::::\n' > "$out/etc/shadow"
  chmod 0600 "$out/etc/shadow"
  touch "$out/etc/machine-id" "$out/etc/resolv.conf"
  mkdir -p "$out/usr/lib/tmpfiles.d"
  printf 'L /var/run - - - - /run\n' > "$out/usr/lib/tmpfiles.d/nickel-container.conf"
  printf 'hosts: files dns\npasswd: files\ngroup: files\n' > "$out/etc/nsswitch.conf"
  printf '127.0.0.1 localhost\n::1 localhost\n' > "$out/etc/hosts"
  cat > "$out/etc/nix/nix.conf" <<'EOF'
  store = local
  build-users-group =
  substituters =
  flake-registry =
  experimental-features = nix-command flakes
  sandbox = true
  accept-flake-config = false
  EOF
  mkdir -p "$out/etc/ssl/certs"
  ln -s ${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt "$out/etc/ssl/certs/ca-certificates.crt"
  cat > "$out/usr/lib/os-release" <<'EOF'
  ID=nickel-plane
  NAME="Nickel-plane system container"
  VERSION_ID=${profile.revision}
  EOF
  ln -s ../usr/lib/os-release "$out/etc/os-release"
  cp ${pkgs.writeText "container-profile.json" (builtins.toJSON profile)} "$out/usr/share/nickel-container/profile.json"
  cp ${closure}/registration "$out/usr/share/nickel-container/registration"
  python3 - ${closure}/store-paths "$out/usr/share/nickel-container/closure.json" <<'PY'
  import json, pathlib, sys
  pathlib.Path(sys.argv[2]).write_text(json.dumps(pathlib.Path(sys.argv[1]).read_text().splitlines(), indent=2) + '\n')
  PY
  chmod 1777 "$out/tmp"
''
