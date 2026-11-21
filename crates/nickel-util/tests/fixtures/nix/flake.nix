{
  description = "Pinned evaluator for nickel-k8s application manifests";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/1c89083e451e891152365b799595cf7790a43325";
  outputs = { nixpkgs, ... }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
  in {
    packages.${system}.nickel = pkgs.nickel;
    devShells.${system}.default = pkgs.mkShell {
      packages = [ pkgs.nickel pkgs.rustc pkgs.cargo pkgs.clippy pkgs.rustfmt pkgs.bubblewrap pkgs.util-linux ];
    };
  };
}
