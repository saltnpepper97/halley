self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.halley;
in
{
  options.programs.halley = {
    enable = lib.mkEnableOption "the Halley Wayland compositor";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.halley;
      defaultText = lib.literalExpression "halley.packages.\${pkgs.stdenv.hostPlatform.system}.halley";
      description = "The Halley package to use for the compositor, session and portal.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    services.displayManager.sessionPackages = [ cfg.package ];
    systemd.packages = [ cfg.package ];
    services.dbus.packages = [ cfg.package ];

    # Preserve the login environment imported by halley-session. Never restart
    # a running desktop as a side effect of nixos-rebuild switch.
    systemd.user.services.halley = {
      restartIfChanged = false;
      enableDefaultPath = false;
    };

    hardware.graphics.enable = lib.mkDefault true;
    services.graphical-desktop.enable = true;
    security.polkit.enable = true;
    security.pam.services.swaylock = { };
    programs.dconf.enable = lib.mkDefault true;
    services.xserver.desktopManager.runXdgAutostartIfNone = lib.mkDefault true;

    services.pipewire = {
      enable = lib.mkDefault true;
      wireplumber.enable = lib.mkDefault true;
    };

    xdg.portal = {
      enable = lib.mkDefault true;
      extraPortals = [
        cfg.package
        pkgs.xdg-desktop-portal-gtk
      ];
      configPackages = [ cfg.package ];
    };
  };
}
