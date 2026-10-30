#!/usr/bin/env python3
"""Manual Orca integration check in private session and accessibility buses.

Set HALLEY_TEST_WINIT_SERVER to the built nested_fullscreen_server executable.
Requires Orca, Python GI/AT-SPI, XWayland, and a running Wayland desktop.
"""
import os
import re
import signal
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def dbus(method, *args):
    return subprocess.run(
        ["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus", "--object-path", "/org/freedesktop/DBus", "--method", method, *args],
        capture_output=True, text=True,
    )


def owner():
    result = dbus("org.freedesktop.DBus.GetNameOwner", "org.gnome.Orca.KeyboardMonitor")
    match = re.search(r"'(:[0-9.]+)'", result.stdout)
    return match.group(1) if match else None


def owner_pid(name):
    result = dbus("org.freedesktop.DBus.GetConnectionUnixProcessID", name)
    match = re.search(r"uint32 (\d+)", result.stdout)
    return int(match.group(1)) if match else None


def wait_for(description, probe, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = probe()
        if value:
            return value
        time.sleep(0.1)
    raise RuntimeError(f"timed out waiting for {description}")


def run_session(runtime):
    runtime = Path(runtime)
    config = runtime / "halley.rune"
    debug = runtime / "orca.log"
    wrapper = runtime / "orca-test"
    wrapper.write_text(f"#!/bin/sh\nexec /usr/bin/orca --debug-file '{debug}'\n")
    wrapper.chmod(0o700)
    config.write_text(f'env:\n  HALLEY_ORCA_PATH "{wrapper}"\nend\nkeybinds:\nend\n')
    env = os.environ.copy()
    env.pop("HALLEY_ORCA_PATH", None)
    env.update(HALLEY_TEST_WINIT_CONFIG=str(config), HALLEY_TEST_WINIT_CONTROL=str(runtime / "input.sock"), HALLEY_TEST_ACCESSIBILITY="1")
    registry = subprocess.Popen(["/usr/lib/at-spi2-registryd"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    compositor = None
    reader_pid = None
    try:
        subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus", "--object-path", "/org/a11y/bus", "--method", "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", "IsEnabled", "<true>"], env=env, check=True, stdout=subprocess.DEVNULL)
        with (runtime / "halley.log").open("w") as log:
            compositor = subprocess.Popen([env["HALLEY_TEST_WINIT_SERVER"]], env=env, stdout=log, stderr=log)
            first = wait_for("Orca keyboard monitor", owner)
            reader_pid = owner_pid(first)
            if not reader_pid:
                raise RuntimeError("could not determine Orca's process ID")
            denied = subprocess.run(["gdbus", "call", "--session", "--dest", "org.freedesktop.a11y.Manager", "--object-path", "/org/freedesktop/a11y/Manager", "--method", "org.freedesktop.a11y.KeyboardMonitor.WatchKeyboard"], env=env, capture_output=True, text=True)
            if denied.returncode == 0 or "AccessDenied" not in denied.stderr:
                raise RuntimeError("unrelated client was not denied keyboard monitoring")
            wait_for("nested input socket", lambda: (runtime / "input.sock").exists())
            subprocess.run([str(ROOT / "target/debug/halleyctl"), "basics"], env=env, check=True)
            wait_for("spoken basics", lambda: debug.exists() and "SPEECH OUTPUT: 'Halley basics." in debug.read_text(errors="replace"))
            control = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
            control.bind(str(runtime / "input-client.sock"))
            control.connect(str(runtime / "input.sock"))
            control.settimeout(5)
            def key(code, state):
                control.send(f"key {code} {state}".encode())
                control.recv(65536)
            key(1, "down"); key(1, "up")
            capture = subprocess.Popen([str(ROOT / "target/debug/halleyctl"), "capture", "menu", "--silent"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                wait_for("spoken screenshot menu", lambda: "SPEECH OUTPUT: 'Screenshot, region." in debug.read_text(errors="replace"))
                key(106, "down"); key(106, "up")
                wait_for("spoken screenshot selection", lambda: "SPEECH OUTPUT: 'Screenshot, monitor." in debug.read_text(errors="replace"))
                key(1, "down"); key(1, "up")
                capture.wait(timeout=5)
            finally:
                if capture.poll() is None:
                    capture.terminate(); capture.wait(timeout=5)
                control.close()
            config.write_text("keybinds:\n  broken\n")
            subprocess.run([str(ROOT / "target/debug/halleyctl"), "reload"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            wait_for("spoken configuration error", lambda: "SPEECH OUTPUT: 'Current configuration was unable to load properly." in debug.read_text(errors="replace"))
            os.kill(reader_pid, signal.SIGTERM)
            second = wait_for("Orca restart", lambda: (name if (name := owner()) and name != first else None))
            reader_pid = owner_pid(second)
            subprocess.run([str(ROOT / "target/debug/halleyctl"), "quit"], env=env, check=True)
            (runtime / "input-client.sock").unlink(missing_ok=True)
            control = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
            control.bind(str(runtime / "input-client.sock"))
            control.connect(str(runtime / "input.sock"))
            control.settimeout(5)
            control.send(b"key 28 down")
            control.recv(65536)
            control.close()
            compositor.wait(timeout=5)
            reader_pid = None  # The supervisor waited for its child during shutdown.
            print("Orca spoke the basics, screenshot selection and configuration error; unauthorized monitoring was denied and Orca restarted.")
    finally:
        if compositor and compositor.poll() is None:
            compositor.terminate()
            try:
                compositor.wait(timeout=5)
            except subprocess.TimeoutExpired:
                compositor.kill(); compositor.wait()
        if reader_pid:
            try:
                os.kill(reader_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        registry.terminate(); registry.wait(timeout=5)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--session":
        run_session(sys.argv[2])
    else:
        server = os.environ.get("HALLEY_TEST_WINIT_SERVER")
        if not server or not Path(server).is_absolute():
            raise SystemExit("Set HALLEY_TEST_WINIT_SERVER to the absolute nested test executable path")
        display = Path(os.environ["WAYLAND_DISPLAY"])
        if not display.is_absolute():
            display = Path(os.environ["XDG_RUNTIME_DIR"]) / display
        runtime = tempfile.mkdtemp(prefix="halley-orca-test-")
        try:
            env = os.environ.copy()
            env.update(XDG_RUNTIME_DIR=runtime, XDG_CONFIG_HOME=runtime, XDG_STATE_HOME=runtime, GSETTINGS_BACKEND="memory", WAYLAND_DISPLAY=str(display))
            subprocess.run(["dbus-run-session", "--", sys.executable, __file__, "--session", runtime], env=env, check=True)
        finally:
            # The private document portal can briefly leave a disconnected
            # FUSE mount while its bus exits. Wait for teardown before removal.
            for attempt in range(50):
                try:
                    shutil.rmtree(runtime)
                    break
                except OSError:
                    if attempt == 49:
                        raise
                    time.sleep(0.1)
