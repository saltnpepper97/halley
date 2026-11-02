#!/usr/bin/env bash
set -euo pipefail

build_dir=${1:-target/release}
output_dir=${2:-target/test-build}
revision=${3:-$(git rev-parse HEAD)}
if [[ ! $revision =~ ^[0-9a-f]{40}$ ]]; then
    echo 'Expected a full Git commit for the test build.' >&2
    exit 1
fi

name="halley-test-linux-$(uname -m)-${revision:0:12}"
mkdir -p "$output_dir"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
for binary in halley halleyctl xdg-desktop-portal-halley; do
    install -m755 "$build_dir/$binary" "$stage/$name/$binary"
done
install -m755 packaging/wayland-sessions/halley-session "$stage/$name/halley-session"
install -m644 LICENSE "$stage/$name/LICENSE"
install -m644 docs/explicit-sync-testing.md "$stage/$name/EXPLICIT-SYNC-TEST.md"

{
    printf 'Commit: %s\nBuilt: %s\n' "$revision" "$(date -u +%FT%TZ)"
    "$build_dir/halley" --version
    cat /etc/os-release
    ldd --version | sed -n '1p'
    for binary in halley halleyctl xdg-desktop-portal-halley; do
        printf '\nDependencies for %s:\n' "$binary"
        ldd "$build_dir/$binary"
    done
} > "$stage/$name/BUILD-INFO.txt"

cat > "$stage/$name/check-build.sh" <<'SH'
#!/bin/sh
set -eu
cd -- "$(dirname -- "$0")"
for binary in halley halleyctl xdg-desktop-portal-halley; do
    dependencies=$(ldd "./$binary" 2>&1) || {
        printf '%s\n' "$dependencies" >&2
        exit 1
    }
    if printf '%s\n' "$dependencies" | grep -q 'not found'; then
        printf '%s\n' "$dependencies" >&2
        echo 'This build requires libraries unavailable on this system. Use the Nix or source-build option in README.txt.' >&2
        exit 1
    fi
done
./halley --version
./halleyctl --version
head -n2 BUILD-INFO.txt
SH
chmod 755 "$stage/$name/check-build.sh"

cat > "$stage/$name/README.txt" <<EOF
Halley test build: $revision

This archive contains the compositor, CLI, portal executable and session launcher.
No system installation is required. Your installed Halley remains available.
The archive uses the build machine's shared libraries (see BUILD-INFO.txt);
it is not a universal Linux binary. On NixOS, use the Nix option below.

1. Extract this archive into a folder you will keep for the test.
2. Open a terminal there and run: ./check-build.sh
3. Log out of your current desktop. Switch to a text console (for example
   Ctrl+Alt+F3), log in, and enter the extracted folder.
4. Run: ./halley-session
   The launcher selects the halley executable beside it, including when it
   hands the session to an already-installed systemd service.

To test explicit synchronization, use a native session. The nested --winit
backend does not advertise linux-drm-syncobj. See EXPLICIT-SYNC-TEST.md for
the Smudgy / issue #264 reproduction and its current hardware limitations.

To verify the running compositor, run this inside its session:
  readlink /proc/\$(pgrep -n -x halley)/exe
It should name this extracted folder's halley, not the installed executable.
Session logs are normally at \$XDG_RUNTIME_DIR/halley-session.log. Record the
commit above, GPU/driver, Smudgy version, and exact trigger with the result.

After testing, log out and choose your normal installed session again.
Delete the extracted folder to remove this test build. The test session uses
your normal Halley configuration; it does not replace it.

Nix / NixOS (no system rebuild or profile replacement):
  nix build github:saltnpepper97/halley/$revision#halley -o result-halley-test
  ./result-halley-test/bin/halley --version
Then log out, enter a text console, and run:
  ./result-halley-test/bin/halley-session
Nix downloads the pinned dependencies and may build Halley locally.

Other distributions can build the same commit from source using the repository
requirements and cargo build --locked --release --workspace. They can then run
HALLEY_BIN pointing at that build through their existing halley-session launcher.
EOF

tar -C "$stage" -czf "$output_dir/$name.tar.gz" "$name"
(cd "$output_dir" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
printf '%s\n' "$output_dir/$name.tar.gz"
