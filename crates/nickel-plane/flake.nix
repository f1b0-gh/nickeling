{
  description = "Pinned evaluator for nickel-plane composition controller";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/1c89083e451e891152365b799595cf7790a43325";
  outputs = { nixpkgs, ... }: let
    system = "x86_64-linux";
    pkgs = import nixpkgs { inherit system; };
  in {
    packages.${system} = {
      kind = pkgs.kind;
      kubectl = pkgs.kubectl;
      nickel = pkgs.nickel;
      opentofu = pkgs.opentofu;
      opentofu-proxmox = pkgs.opentofu.withPlugins (p: [ p.bpg_proxmox ]);
      opentofu-local = pkgs.opentofu.withPlugins (p: [ p.hashicorp_random ]);
    };
    devShells.${system}.default = pkgs.mkShell {
      packages = [ pkgs.nickel pkgs.opentofu pkgs.rustc pkgs.cargo pkgs.clippy pkgs.rustfmt pkgs.bubblewrap pkgs.util-linux pkgs.openssl ];
    };
  };
}
