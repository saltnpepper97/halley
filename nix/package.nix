{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  bash,
  coreutils,
  dbus,
  gnugrep,
  gnused,
  procps,
  systemd,
  systemdLibs,
  libdisplay-info_0_3,
  libdrm,
  libgbm,
  libglvnd,
  libinput,
  libx11,
  libxi,
  libxkbcommon,
  pipewire,
  seatd,
  wayland,
  xwayland,
}:

rustPlatform.buildRustPackage {
  pname = "halley";
  version = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package.version;

  src = lib.cleanSourceWith {
    src = lib.cleanSource ../.;
    filter = path: type: !(type == "directory" && builtins.baseNameOf path == "target");
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    # Both Smithay crates share this pinned checkout.
    outputHashes."smithay-0.7.0" = "sha256-DUSciVTN5Ds2AYZVaGmMu7DBINRxu3CIdUCT4QCplTY=";
  };

  nativeBuildInputs = [
    pkg-config
    makeWrapper
    rustPlatform.bindgenHook
  ];
  buildInputs = [
    libdisplay-info_0_3
    libdrm
    libgbm
    libglvnd
    libinput
    libx11
    libxi
    libxkbcommon
    pipewire
    seatd
    systemdLibs
    wayland
  ];
  nativeCheckInputs = [
    bash
    coreutils
    dbus
  ];

  cargoBuildFlags = [ "--workspace" ];
  cargoTestFlags = [ "--workspace" ];

  preCheck = ''
    export XDG_RUNTIME_DIR=$(mktemp -d)
  '';

  # Keep dynamically loaded graphics libraries in the runtime closure/RPATH.
  env.RUSTFLAGS = lib.concatStringsSep " " (
    map (arg: "-C link-arg=${arg}") [
      "-Wl,--push-state,--no-as-needed"
      "-lEGL"
      "-lwayland-client"
      "-lX11"
      "-lX11-xcb"
      "-lXi"
      "-lxkbcommon-x11"
      "-Wl,--pop-state"
    ]
  );

  
  postPatch = ''
    # These test fixtures deliberately clear PATH and create executable scripts.
    substituteInPlace tests/session_lifecycle.rs \
      --replace-fail '/bin/sh' '${lib.getExe bash}' \
      --replace-fail '/usr/bin:/bin' '${
        lib.makeBinPath [
          bash
          coreutils
        ]
      }'
    substituteInPlace tests/idle_service.rs halley-portal/tests/portal_dispatch.rs \
      --replace-fail '"--session"' '"--config-file=${dbus}/share/dbus-1/session.conf"'
    substituteInPlace src/accessibility/dbus.rs \
      --replace-fail '"/usr/bin/sleep"' '"${coreutils}/bin/sleep"'
  '';


  postInstall = ''
    install -Dm755 packaging/wayland-sessions/halley-session "$out/bin/halley-session"
    patchShebangs "$out/bin/halley-session"
    substituteInPlace "$out/bin/halley-session" \
      --replace-fail '/usr/bin/halley' "$out/bin/halley"

    install -Dm644 packaging/wayland-sessions/halley.desktop \
      "$out/share/wayland-sessions/halley.desktop"
    substituteInPlace "$out/share/wayland-sessions/halley.desktop" \
      --replace-fail '=halley-session' "=$out/bin/halley-session"

    mkdir -p "$out/share/xdg-desktop-portal" "$out/share/dbus-1/services" \
      "$out/lib/systemd/user"
    cp -r packaging/xdg-desktop-portal/. "$out/share/xdg-desktop-portal/"
    cp packaging/dbus-1/services/*.service "$out/share/dbus-1/services/"
    cp -r packaging/systemd-user/. "$out/lib/systemd/user/"
    substituteInPlace \
      "$out/share/dbus-1/services/org.freedesktop.impl.portal.desktop.halley.service" \
      "$out/lib/systemd/user/xdg-desktop-portal-halley.service" \
      --replace-fail '/usr/bin/xdg-desktop-portal-halley' "$out/bin/xdg-desktop-portal-halley"
    substituteInPlace "$out/lib/systemd/user/halley.service" \
      --replace-fail '/usr/bin/halley-session' "$out/bin/halley-session"

    wrapProgram "$out/bin/halley" \
      --prefix PATH : ${
        lib.makeBinPath [
          bash
          dbus
          systemd
          xwayland
        ]
      }
    wrapProgram "$out/bin/halley-session" \
      --prefix PATH : ${
        lib.makeBinPath [
          coreutils
          dbus
          gnugrep
          gnused
          procps
          systemd
        ]
      }
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    "$out/bin/halley" --version
    "$out/bin/halleyctl" --help
    test -x "$out/bin/xdg-desktop-portal-halley"
    test -x "$out/bin/halley-session"
    grep -F "Exec=$out/bin/halley-session" "$out/share/wayland-sessions/halley.desktop"
    grep -F "ExecStart=$out/bin/halley-session" "$out/lib/systemd/user/halley.service"
    grep -F "ExecStart=$out/bin/xdg-desktop-portal-halley" \
      "$out/lib/systemd/user/xdg-desktop-portal-halley.service"
    runHook postInstallCheck
  '';

  passthru.providedSessions = [ "halley" ];

  meta = {
    description = "Spatial Wayland compositor built around infinite workspace navigation";
    homepage = "https://github.com/saltnpepper97/halley";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "halley";
  };
}
