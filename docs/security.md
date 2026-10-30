# Desktop capability permissions

Halley does not treat permission to display a window as permission to read the
whole desktop or inject keyboard input.

## Accessibility keyboard monitor

Keyboard monitoring is denied by default. To approve a screen reader, first
start it on the session bus, determine its unique connection name using
`busctl --user status org.gnome.Orca.KeyboardMonitor`, and launch Halley with
`HALLEY_ACCESSIBILITY_MONITOR=:1.42` (substitute that connection's unique name).
The approved connection must also own `org.gnome.Orca.KeyboardMonitor`.
A well-known name, wildcard, executable name, or replacement connection is not
an approval. Restarting the screen reader requires a new explicit approval.
Do not set this variable from an untrusted application's output automatically.

For a restart-friendly Orca session, set `HALLEY_ORCA_PATH` to the absolute
path of the trusted Orca executable before starting Halley, for example
`HALLEY_ORCA_PATH=/usr/bin/orca halley`. Halley starts and supervises that
process and approves its live D-Bus connection only while that exact child is
running and owns `org.gnome.Orca.KeyboardMonitor`. If Orca exits, Halley
starts it again and approves the replacement process. Halley stops its child
when the session exits. Do not configure an interpreter, shell, generic
launcher, or untrusted executable: the selected program receives keyboard
monitoring authority. Avoid starting a second Orca instance independently;
the supervised process needs to be the instance that owns the bus name.

For a display-manager session, the same opt-in can be placed in Halley's
configuration and takes effect at the next login:

```rune
env:
  HALLEY_ORCA_PATH "/usr/bin/orca"
end
```

Install Orca, Speech Dispatcher, a speech synthesizer, and the desktop's
AT-SPI services using your distribution's packages. The path may differ on
your system. To turn this off, remove the setting and restart the session.

These controls constrain protocol access, not arbitrary execution as your Unix
user. Applications able to modify your session environment or compositor
configuration already share your desktop's administrative trust boundary.

## Portal backend

Portal capture uses Halley's normal source chooser and consent flow. Backend
requests do not require a particular frontend bus name or executable path.
Requests and sessions remain bound to the initiating unique bus connection;
sessions also retain their app ID. A different connection cannot reuse or
close another connection's session.

## Sensitive Wayland protocols

All ordinary connections start without desktop-wide capabilities. Before
launching Halley, set a colon-separated list of **absolute executable paths**
for each capability you intend to grant:

| Environment variable | Capability | Example trusted tool |
| --- | --- | --- |
| `HALLEY_ALLOW_SCREEN_CAPTURE` | Direct wlr-screencopy and ext image-copy capture | `/usr/bin/grim` |
| `HALLEY_ALLOW_CLIPBOARD_CONTROL` | Clipboard/primary-selection monitoring | `/usr/bin/wl-paste` |
| `HALLEY_ALLOW_VIRTUAL_KEYBOARD` | Synthetic keyboard input | `/usr/bin/wtype` |
| `HALLEY_ALLOW_INPUT_METHOD` | IME registration and keyboard grabs | `/usr/bin/fcitx5` |
| `HALLEY_ALLOW_OUTPUT_POWER` | Native display DPMS control and observation | `/usr/bin/wlopm` |

The capture grant controls visibility of `zwlr_screencopy_manager_v1`,
`ext_image_copy_capture_manager_v1`, and
`ext_output_image_capture_source_manager_v1`. The ext protocols use the same
executable identity checks and do not grant a new client capture access.

For example: `HALLEY_ALLOW_INPUT_METHOD=/usr/bin/fcitx5 halley`.
For standard output power tools, approve the installed executable with
`HALLEY_ALLOW_OUTPUT_POWER=/usr/bin/wlopm` before starting the native session.
This grants visibility of `zwlr_output_power_manager_v1`; the nested backend
never creates this global. See [output power management](output-power.md).
There are no implicit grants, basename matches, or wildcards. The compositor
compares the connecting peer's executable device/inode against the approved
files. Grants are fixed for that connection. Restart Halley and reconnect the
tool after changing the policy. A copied executable does not inherit a grant.
Never approve a shell, interpreter, generic application launcher, or sandbox
broker: its children or delegated sockets may carry its authority. Approved
programs themselves must be trusted, including their plugins and environment.

Normal focused clipboard copy/paste and text-input clients remain available.
Waybar's ordinary panel/tray operation needs none of these grants. Portal
capture uses Halley's separate compositor IPC, so applications should prefer
the portal's consent flow over unrestricted raw capture. The private same-user
IPC socket remains a trusted desktop administration interface; do not expose
it to untrusted sandboxes. This is not isolation from arbitrary unsandboxed
programs that can execute code as your user or modify your session setup.

## IPC resource limits

The private compositor IPC admits at most 64 concurrent connections. Each
request frame must arrive within 30 seconds (a slow byte stream does not extend
that deadline); writes time out after 5 seconds, and deferred request replies
expire after 5 minutes. Subscriptions may remain open while idle but are
removed on disconnect. An idle ordinary connection expires after 30 seconds;
clients must reconnect rather than reuse an expired connection.

DMA-BUF registrations are scoped to the IPC connection: 16 buffers per
connection and 128 total, at most four planes/descriptors per buffer. Closing
the connection releases all its registrations. Capture and removal requests
cannot access another connection's registered buffers, even if the stream name
and buffer ID are identical. Persistent recording connections keep their own
registrations while requesting frames.
