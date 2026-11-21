{ pkgs, profile }:
let
  inherit (pkgs) lib;
  # Snapshot only manifests and build inputs, excluding state, credentials and
  # Cargo outputs. The root manifest and lock define the workspace graph.
  crate = path: lib.fileset.unions ([ (path + /Cargo.toml) (path + /src) ]
    ++ lib.optional (builtins.pathExists (path + /nickel)) (path + /nickel));
  sources = lib.fileset.toSource {
    root = ../../../..;
    fileset = lib.fileset.unions [
      ../../../../Cargo.toml ../../../../Cargo.lock
      (crate ../..)
      (crate ../../../nickel-tf)
      (crate ../../../nickel-k8s)
      (crate ../../../nickel-util)
    ];
  };
  plane = pkgs.rustPlatform.buildRustPackage {
    pname = "nickel-plane";
    version = "0.1.0";
    src = sources;
    cargoBuildFlags = [ "--package" "nickel-plane" ];
    cargoLock.lockFile = ../../../../Cargo.lock;
    # Nix builds variants sequentially, with at most two compiler jobs each.
    enableParallelBuilding = false;
    env.CARGO_BUILD_JOBS = "2";
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.sqlite ];
    env.LIBSQLITE3_SYS_USE_PKG_CONFIG = "1";
    buildNoDefaultFeatures = true;
    buildFeatures = [ "cli" ] ++ profile.adapters;
    # Domain/controller behavior has its own test suite; this derivation tests
    # packaging through the container fixture, without invoking host resources.
    doCheck = false;
    meta.mainProgram = "nickel-plane";
  };
  resources = name: path: pkgs.runCommand "nickel-${name}-container-resources" {} ''
    mkdir -p "$out/provenance"
    cp ${path + /flake.nix} "$out/provenance/flake.nix"
    cp ${path + /flake.lock} "$out/provenance/flake.lock"
    cp -r ${path + /nickel} "$out/nickel"
    # The immutable runtime only needs this already-resolved output. Keep Nix's
    # real offline flake evaluation and store query, without shipping build inputs.
    cat > "$out/flake.nix" <<'EOF'
    {
      description = "Frozen nickel-${name} evaluator from locked container build";
      outputs = { self }: {
        packages.x86_64-linux.nickel.outPath = "${pkgs.nickel}";
      };
    }
    EOF
    cat > "$out/flake.lock" <<'EOF'
    {"nodes":{"root":{"inputs":{}}},"root":"root","version":7}
    EOF
  '';
in {
  inherit plane;
  planeResources = resources "plane" ../..;
  tfResources = resources "tf" ../../../nickel-tf;
  k8sResources = resources "k8s" ../../../nickel-k8s;
}
