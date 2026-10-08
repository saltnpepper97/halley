# Autostart commands and logs

`autostart.once` commands run once when a full TTY session starts, after
Halley's display sockets are ready. `autostart.on-reload` commands run after a
valid configuration reload. Nested Winit sessions do not run either group.
Repeated `once` entries with identical command lines are launched only once,
ignoring surrounding whitespace and preserving first-entry order. Different
arguments remain separate commands. `on-reload` remains an explicit instruction
to rerun a command on every accepted reload, including duplicate entries;
avoid putting a long-running service there unless it manages its own restart.

```rune
autostart:
  once "waybar"
  once "mako"
end
```

Each command gets a persistent output log under
`$XDG_STATE_HOME/halley/autostart`, falling back to
`~/.local/state/halley/autostart`. The filename starts with the executable name
and includes a stable hash of the full command line, so different arguments
get distinct files. Halley's session log prints the exact path at launch.

For example, to inspect Waybar failures after logging in:

```sh
tail -n 100 ~/.local/state/halley/autostart/waybar-*.log
```

Logs contain the launch time, command, selected display sockets, merged standard
output and error, and the shell's exit status. Logging is always enabled for
autostart commands; add a command's own debug flag when more detail is needed.
Normal keybind launches retain their existing output behavior.

When an autostart exits unsuccessfully, Halley reports its command, exit status,
and output-log path once in the session log. Successful one-shot commands remain
debug messages; normal owned shutdown does not produce failure warnings. This
reports exits without restarting commands automatically. Failed session
integration helpers also include their stderr context, limited to 4 KiB of
reported detail; optional helper failures retain their debug severity.

Each command retains its current log and two older generations (`.log.1` and
`.log.2`), at most 1 MiB each. Reloads and repeated logins append to the same
bounded files. The log directory is private to the user (mode 0700), and files
use mode 0600. Logs persist across compositor restarts and reboots.

A small `halley-autolog` process captures each command's output without creating
another compositor. If log storage is unavailable, the command still starts.
Autostart is a convenience launcher: use the user service manager for services
that need restart policies or supervision across crashes.

On a clean native logout, Halley sends SIGTERM to the startup command's process
group while the display is still available. It drains final log output, waits
up to one second for all commands together, then terminates unresponsive groups.
Only groups launched by that compositor instance are included. Keybind-launched
applications and externally managed services keep their own lifecycle. Programs
that deliberately daemonize into a new session should use the user service
manager, which can supervise their complete process tree.

For managed desktop services, bind their lifetime and startup ordering to the
graphical session. For example:

```ini
[Unit]
PartOf=graphical-session.target
Requisite=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/waybar
Restart=on-failure
```

Start the unit through `halley.service.wants/` for managed sessions or
`halley-direct-session.target.wants/` for direct sessions. Remove the equivalent
`autostart.once` command when adopting a service. Do not start the same tray
service through both paths. Services launched outside the graphical session can
survive logout and reject the next session's launch as an existing instance.
