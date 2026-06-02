{
  description = "Halley - Spatial Wayland compositor built around infinite workspace navigation";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
      in
      {
        packages = {
          halley = pkgs.rustPlatform.buildRustPackage rec {
            pname = "halley";
            version = "0.3.2";

            src = self;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            buildInputs = with pkgs; [
              pkg-config
              libxkbcommon
              wayland
              libinput
              libseat
              xwayland
              udev
            ];

            nativeBuildInputs = with pkgs; [
              pkg-config
              rustPlatform.bindgenHook
            ];

            postInstall = ''
              # Install session file for display managers
              install -Dm755 $src/packaging/wayland-sessions/halley-session $out/bin/halley-session
              install -Dm644 $src/packaging/wayland-sessions/halley.desktop $out/share/wayland-sessions/halley.desktop
              
              # Install systemd user units
              install -Dm644 $src/packaging/systemd-user/halley.service $out/lib/systemd/user/halley.service
              install -Dm644 $src/packaging/systemd-user/halley-shutdown.target $out/lib/systemd/user/halley-shutdown.target
            '';

            meta = with pkgs.lib; {
              description = "Spatial Wayland compositor built around infinite workspace navigation";
              homepage = "https://github.com/CG-GeisT/halley";
              license = licenses.gpl3Only;
              maintainers = [];
              platforms = platforms.linux;
            };
          };

          halley-aperture = pkgs.rustPlatform.buildRustPackage rec {
            pname = "halley-aperture";
            version = "0.3.2";

            src = self;
            sourceRoot = "source/crates/halley-aperture";

            cargoLock = {
              lockFile = ../../Cargo.lock;
            };

            buildInputs = with pkgs; [
              pkg-config
              libxkbcommon
              wayland
            ];

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            meta = with pkgs.lib; {
              description = "A status bar and utility for the Halley Wayland compositor";
              homepage = "https://github.com/CG-GeisT/halley";
              license = licenses.gpl3Only;
              maintainers = [];
              platforms = platforms.linux;
            };
          };

          default = self.packages.${system}.halley;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rust-analyzer
            pkg-config
            libxkbcommon
            wayland
            libinput
            libseat
            xwayland
            udev
            clippy
            rustfmt
          ];

          shellHook = ''
            echo "Halley development environment loaded"
          '';
        };

        checks.build = self.packages.${system}.halley;
      }
    );
}
