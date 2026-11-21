{ pkgs, profile }:
let
  accepts = selection: (builtins.tryEval ((import ../nix/rootfs.nix {
    inherit pkgs;
    profile = selection;
    packages = import ../nix/packages.nix { inherit pkgs; profile = selection; };
  }).drvPath)).success;
  rejects = bad: !(builtins.tryEval ((import ../nix/rootfs.nix {
    inherit pkgs;
    profile = bad;
    packages = {};
  }).drvPath)).success;
  cases = [
    (profile // { version = 2; })
    (profile // { architecture = "aarch64_linux"; })
    (profile // { revision = "../bad"; })
    (profile // { adapters = []; })
    (profile // { adapters = [ "opentofu" "opentofu" ]; })
    (profile // { adapters = [ "shell" ]; })
    (profile // { kernel = "optional"; })
    (builtins.removeAttrs profile [ "revision" ])
    (profile // { version = "1"; })
    (profile // { version = 1.0; })
    (profile // { architecture = null; })
    (profile // { revision = 1; })
    (profile // { adapters = "opentofu"; })
    (profile // { adapters = [ null ]; })
    (profile // { adapters = [ {} ]; })
    null
    []
  ];
in assert pkgs.lib.all accepts [
  profile
  (profile // { adapters = [ "opentofu" ]; })
  (profile // { adapters = [ "kubernetes" ]; })
];
assert pkgs.lib.all rejects cases;
pkgs.runCommand "nickel-container-profile-refusals" {} "touch $out"
