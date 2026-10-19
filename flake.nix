{
  description = "Halley spatial Wayland compositor";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      overlays.default = final: _prev: {
        halley = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ self.overlays.default ];
          };
        in
        {
          inherit (pkgs) halley;
          default = pkgs.halley;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.halley}/bin/halley";
          meta.description = "Halley spatial Wayland compositor";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          package = self.packages.${system}.halley;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ package ];
            RUSTFLAGS = package.RUSTFLAGS;
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rustfmt
              pkgs.clippy
            ];
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) halley;
        nixos-module = import ./nix/module-check.nix { inherit nixpkgs system self; };
      });

      nixosModules.default = import ./nix/module.nix self;
      nixosModules.halley = self.nixosModules.default;
    };
}
