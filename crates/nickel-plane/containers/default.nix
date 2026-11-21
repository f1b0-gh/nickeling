{ profile ? builtins.fromJSON (builtins.readFile ./profiles/plane.json) }:
let
  # Reuse the evaluator lock; all source files live in this workspace.
  pinned = builtins.getFlake ("path:" + toString ../.);
  pkgs = pinned.inputs.nixpkgs.legacyPackages.x86_64-linux;
  mk = selection: import ./nix/rootfs.nix {
    inherit pkgs;
    profile = selection;
    packages = import ./nix/packages.nix { inherit pkgs; profile = selection; };
  };
  rootfs = mk profile;
  tfRootfs = mk (profile // { adapters = [ "opentofu" ]; });
  oci = import ./nix/oci.nix { inherit pkgs rootfs; };
  tfOci = import ./nix/oci.nix { inherit pkgs; rootfs = tfRootfs; };
  checkOci = name: output: imageRootfs: pkgs.runCommand name { nativeBuildInputs = [ pkgs.python3 ]; } ''
    python3 ${./tests/check_oci.py} --archive ${output.image} --rootfs ${imageRootfs} \
      --quadlet ${output.quadlet} --generator ${pkgs.podman}/libexec/podman/quadlet -v
    touch "$out"
  '';
  check = name: output: adapters: pkgs.runCommand name { nativeBuildInputs = [ pkgs.python3 ]; } ''
    python3 ${./tests/check_rootfs.py} --rootfs ${output} --adapters ${pkgs.lib.escapeShellArg adapters} -v
    touch "$out"
  '';
in {
  container-rootfs = rootfs;
  container-rootfs-tf = tfRootfs;
  oci-image = oci.image;
  oci-image-tf = tfOci.image;
  quadlet = oci.quadlet;
  quadlet-tf = tfOci.quadlet;
  qualification-host = import ./nix/qualification.nix { inherit pkgs rootfs; };
  qualification-podman = import ./nix/qualification-podman.nix { inherit pkgs oci; tfOci = tfOci; };
  qualification-tools = { qemu = pkgs.qemu_kvm; mkfs = pkgs.e2fsprogs; };
  packages = import ./nix/packages.nix { inherit pkgs profile; };
  checks = {
    contracts = import ./tests/profile_contracts.nix { inherit pkgs profile; };
    layout = check "nickel-container-layout" rootfs (pkgs.lib.concatStringsSep "," profile.adapters);
    tf-layout = check "nickel-container-tf-layout" tfRootfs "opentofu";
    oci = checkOci "nickel-container-oci" oci rootfs;
    tf-oci = checkOci "nickel-container-tf-oci" tfOci tfRootfs;
    profile = pkgs.runCommand "nickel-container-profile" { nativeBuildInputs = [ pkgs.nickel pkgs.python3 ]; } ''
      nickel export --format json ${./profiles}/plane.ncl > actual.json
      python3 - <<'PY'
      import json
      assert json.load(open('actual.json')) == json.load(open('${./profiles/plane.json}'))
      PY
      touch "$out"
    '';
  };
}
