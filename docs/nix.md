# Nix and NixOS

The flake provides `packages.<system>.halley` (also the default package), a
development shell, an overlay, and an optional NixOS module. It supports
`x86_64-linux` and `aarch64-linux`. The package builds the complete workspace
with the default features: `halley`, `halleyctl`, and
`xdg-desktop-portal-halley`, plus the session launcher and desktop integration
resources. XWayland is included in Halley's executable search path.

## Build or try the package

Enable Nix's `nix-command` and `flakes` experimental features, then from a
checkout run:

```sh
nix build
./result/bin/halley --version
./result/bin/halleyctl --help
```

Inside an existing Wayland desktop, run a nested compositor with:

```sh
nix run . -- --winit
```

Install the package in a user profile with `nix profile add .`. Installing the
package alone does not register a display-manager session or configure system
portals. On NixOS, use the module below for a native desktop session.

Halley Lift is a separate project and is not included. Install it separately
for the default launcher binding, or change that binding in your Halley
configuration to a launcher you already have. The same applies to your
terminal, bar, notification daemon, and authentication agent.

## NixOS module

Add Halley to your system flake and import its module in your host definition:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    halley = {
      url = "github:saltnpepper97/halley";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, halley, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        halley.nixosModules.default
        { programs.halley.enable = true; }
      ];
    };
  };
}
```

Rebuild your system, then select **Halley** in your display manager. The
module registers the session and systemd user units, enables graphics and
Polkit support, and enables PipeWire and WirePlumber by default for screen
sharing. It configures Halley's ScreenCast/Screenshot portal alongside the GTK
fallback for dialogs and settings. Keep your existing display-manager setup;
the module does not choose one. From a TTY, `halley-session` starts a native
session too.

You can override `programs.halley.package` with another compatible Halley
derivation. The overlay is available as `halley.overlays.default` if you prefer
to access the package as `pkgs.halley`.

The module does not write or replace `~/.config/halley/config.rune`. Halley
uses its normal first-run configuration behavior. The session launcher uses
the user's login-shell environment for application discovery.

## Development and validation

```sh
nix develop
cargo check --workspace --locked
```

`nix flake check` builds the package, runs the workspace's normal tests, and
checks the NixOS module's enabled configuration. Tests that explicitly require
a live compositor or GPU remain ignored. Run `nix flake check --all-systems`
only with builders available for both supported architectures.

`flake.lock` pins nixpkgs and `Cargo.lock` pins Rust dependencies. Update the
nixpkgs pin with `nix flake update nixpkgs`. When the Smithay Git revision
changes in `Cargo.lock`, update its fixed-output hash in `nix/package.nix` too.
