pub const HELP: &str = "\
Usage: halleyctl <command>

Commands:
  outputs        List connected monitors and their current mode/position
  reload         Reload the selected configuration immediately
  basics         Show Halley's one-time basics card again
  capture        Enter Halley's native screenshot capture modes
  dpms           Control tty output power state
  node           List, inspect, focus, move, collapse, restore, toggle, or close nodes
  cluster        List, inspect, switch, or change optional cluster workspaces
  bearings       Show, hide, toggle, or inspect Bearings (offscreen spatial retrieval)
  trail          Navigate or inspect this monitor's recent-work focus history
  pan            Pan the selected Field: left|right|up|down
  monitor        Focus a monitor or transfer the selected Field window
  stack          Cycle an active stacking cluster
  tile           Focus or swap cluster tiles
  portal         Inspect the desktop portal backend
  config         Edit, migrate, or verify the selected configuration
  quit           Open Halley's exit confirmation

Retrieval in the compositor:
  Mod+Arrow      nearby spatial navigation
  Alt+Tab        recent-work navigation
  Bearings       retrieval for offscreen spatial work
  Apogee         visual inventory across monitors
  Lift           direct search by application, node, cluster, or action

Options:
  -h, --help     Print this message
  -V, --version  Print both halleyctl's and the running compositor's version
";

pub const CLUSTER_HELP: &str = "\
Usage:
  halleyctl cluster list [-o OUTPUT] [--json]
  halleyctl cluster info [current|ID|id:ID] [-o OUTPUT] [--json]
  halleyctl cluster layout-cycle [-o OUTPUT]
  halleyctl cluster slot 1..10 [-o OUTPUT]

Without -o, current and control commands use the selected monitor.
";

pub const BASICS_HELP: &str = "\
Usage:
  halleyctl basics

Shows Halley's basics card again on the selected monitor. The card is offered
automatically once to a freshly generated configuration's first native session;
the card is dismissed with Enter, Escape, or a click.
";

pub const CAPTURE_HELP: &str = "\
Usage:
  halleyctl capture menu [-o OUTPUT]
  halleyctl capture region [-o OUTPUT]
  halleyctl capture screen [-o OUTPUT]
  halleyctl capture window [-o OUTPUT]

The command waits until the capture is saved or cancelled.
";

pub const CONFIG_HELP: &str = "\
Usage:
  halleyctl config edit
  halleyctl config edit -c PATH
  halleyctl config edit --config PATH
  halleyctl config verify
  halleyctl config verify -c PATH
  halleyctl config verify --config PATH
  halleyctl config migrate [--dry-run]
  halleyctl config migrate [--dry-run] -c PATH
  halleyctl config migrate [--dry-run] --config PATH

`edit` uses $VISUAL, then $EDITOR, and falls back to vi.
`migrate` explicitly applies structurally detected compatibility updates. It
validates the complete result and keeps a timestamped backup. Pre-0.6 files
require replacement with the current default. Use --dry-run to inspect first.
";

pub const NODE_HELP: &str = "\
Usage:
  halleyctl node list [-o OUTPUT] [--json]
  halleyctl node info [SELECTOR] [-o OUTPUT] [--json]
  halleyctl node focus [SELECTOR] [-o OUTPUT]
  halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]
  halleyctl node collapse [SELECTOR] [-o OUTPUT]
  halleyctl node restore [SELECTOR] [-o OUTPUT]
  halleyctl node toggle [SELECTOR] [-o OUTPUT]
  halleyctl node close [SELECTOR] [-o OUTPUT]

Selectors:
  focused, latest, ID, id:ID, title:TEXT, app:APP_ID

Markers:
  * focused node
  + latest node
  - other node
";

pub const BEARINGS_HELP: &str = "\
Usage:
  halleyctl bearings show
  halleyctl bearings hide
  halleyctl bearings toggle
  halleyctl bearings status

Bearings is Halley's retrieval overlay for offscreen spatial work on each
monitor. Nearby spatial navigation stays on Mod+Arrow, recent-work navigation on
Alt+Tab, the multi-monitor visual inventory on Apogee, and direct search on
Halley Lift.
";

pub const TRAIL_HELP: &str = "\
Usage:
  halleyctl trail prev [-o OUTPUT]
  halleyctl trail next [-o OUTPUT]
  halleyctl trail list [-o OUTPUT] [--json]
  halleyctl trail goto INDEX|SELECTOR [-o OUTPUT]

Trail walks each monitor's recent Field focus history: prev and next move through
it, list prints it, and goto selects an entry directly. It exposes the same
recent work that Alt+Tab cycles as a carousel, with explicit backward/forward
control and scripting.

Selectors:
  focused, latest, ID, id:ID, title:TEXT, app:APP_ID
";

pub const MONITOR_HELP: &str = "\
Usage:
  halleyctl monitor focus left|right|up|down|OUTPUT
  halleyctl monitor transfer left|right|up|down
";

pub const STACK_HELP: &str = "\
Usage:
  halleyctl stack cycle forward [-o OUTPUT]
  halleyctl stack cycle backward [-o OUTPUT]
";

pub const TILE_HELP: &str = "\
Usage:
  halleyctl tile focus left|right|up|down [-o OUTPUT]
  halleyctl tile swap left|right|up|down [-o OUTPUT]
";

pub const PORTAL_HELP: &str = "\
Usage:
  halleyctl portal status [--json]
  halleyctl portal version [--json]
";

pub const DPMS_HELP: &str = "\
Usage:
  halleyctl dpms off|on|toggle [-o OUTPUT]
";
