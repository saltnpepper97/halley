use crate::parse::UsageError;

#[derive(Clone, Copy)]
pub(crate) enum HelpTopic {
    Top,
    Quit,
    Reload,
    Outputs,
    Capture,
    Dpms,
    Node,
    NodeList,
    NodeInfo,
    NodeFocus,
    NodeMove,
    NodeClose,
    Trail,
    TrailPrev,
    TrailNext,
    TrailList,
    TrailGoto,
    Monitor,
    MonitorFocus,
    Bearings,
    BearingsShow,
    BearingsHide,
    BearingsToggle,
    BearingsStatus,
    Cluster,
    ClusterList,
    ClusterInspect,
    ClusterLayout,
    ClusterLayoutCycle,
    Stack,
    StackCycle,
    Tile,
    TileFocus,
    TileSwap,
}

pub(crate) fn exit_usage(error: UsageError) -> ! {
    eprintln!("{}", error.message);
    eprintln!();
    print_help(error.help);
    std::process::exit(2);
}

pub(crate) fn print_help(topic: HelpTopic) {
    match topic {
        HelpTopic::Top => print_help_page(
            "halleyctl",
            &["halleyctl <command> [args]"],
            &[
                ("quit", "Ask the running Halley compositor to exit"),
                (
                    "reload",
                    "Ask the running Halley compositor to reload config",
                ),
                ("outputs", "Print current output information"),
                ("capture", "Enter Halley-native screenshot capture mode"),
                ("dpms", "Control output power state"),
                ("node", "Node actions and inspection"),
                ("trail", "Trail navigation and inspection"),
                ("monitor", "Monitor-related actions"),
                ("bearings", "Bearings visibility controls"),
                ("cluster", "Cluster workspace actions"),
                ("stack", "Stack layout actions"),
                ("tile", "Tile layout actions"),
            ],
        ),
        HelpTopic::Quit => print_help_page(
            "halleyctl quit",
            &["halleyctl quit"],
            &[("quit", "Ask the running Halley compositor to exit")],
        ),
        HelpTopic::Reload => print_help_page(
            "halleyctl reload",
            &["halleyctl reload"],
            &[(
                "reload",
                "Ask the running Halley compositor to reload config",
            )],
        ),
        HelpTopic::Outputs => print_help_page(
            "halleyctl outputs",
            &["halleyctl outputs"],
            &[("outputs", "Print current output information")],
        ),
        HelpTopic::Capture => print_help_page(
            "halleyctl capture",
            &[
                "halleyctl capture menu",
                "halleyctl capture region [-o OUTPUT]",
                "halleyctl capture screen [-o OUTPUT]",
                "halleyctl capture window [-o OUTPUT]",
            ],
            &[
                ("menu", "Open the Halley-native capture menu"),
                ("region", "Start region capture mode"),
                ("screen", "Start screen capture mode"),
                ("window", "Start window capture mode"),
                ("-o OUTPUT", "Start capture mode on a specific output"),
            ],
        ),
        HelpTopic::Dpms => print_help_page(
            "halleyctl dpms",
            &["halleyctl dpms off|on|toggle [-o OUTPUT]"],
            &[("off|on|toggle", "Set or toggle output power state")],
        ),
        HelpTopic::Node => print_help_page(
            "halleyctl node",
            &[
                "halleyctl node list [-o OUTPUT] [--json]",
                "halleyctl node info [SELECTOR] [-o OUTPUT] [--json]",
                "halleyctl node focus [SELECTOR] [-o OUTPUT]",
                "halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]",
                "halleyctl node close [SELECTOR] [-o OUTPUT]",
            ],
            &[
                ("list", "List nodes"),
                ("info", "Show information about a node"),
                ("focus", "Focus a node by selector"),
                ("move", "Move a node in a direction"),
                ("close", "Close a node"),
            ],
        ),
        HelpTopic::NodeList => print_help_page(
            "halleyctl node list",
            &["halleyctl node list [-o OUTPUT] [--json]"],
            &[("list", "List nodes on the focused or selected output")],
        ),
        HelpTopic::NodeInfo => print_help_page(
            "halleyctl node info",
            &["halleyctl node info [SELECTOR] [-o OUTPUT] [--json]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::NodeFocus => print_help_page(
            "halleyctl node focus",
            &["halleyctl node focus [SELECTOR] [-o OUTPUT]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::NodeMove => print_help_page(
            "halleyctl node move",
            &["halleyctl node move left|right|up|down [SELECTOR] [-o OUTPUT]"],
            &[
                ("left|right|up|down", "Direction to move the node"),
                (
                    "SELECTOR",
                    "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
                ),
            ],
        ),
        HelpTopic::NodeClose => print_help_page(
            "halleyctl node close",
            &["halleyctl node close [SELECTOR] [-o OUTPUT]"],
            &[(
                "SELECTOR",
                "Use focused, latest, id:<id>, title:<text>, or app:<app-id>",
            )],
        ),
        HelpTopic::Trail => print_help_page(
            "halleyctl trail",
            &[
                "halleyctl trail prev [-o OUTPUT]",
                "halleyctl trail next [-o OUTPUT]",
                "halleyctl trail list [-o OUTPUT] [--json]",
                "halleyctl trail goto <TARGET> [-o OUTPUT]",
            ],
            &[
                ("prev", "Focus the previous trail entry"),
                ("next", "Focus the next trail entry"),
                ("list", "List trail entries"),
                ("goto", "Focus a specific trail entry"),
            ],
        ),
        HelpTopic::TrailPrev => print_help_page(
            "halleyctl trail prev",
            &["halleyctl trail prev [-o OUTPUT]"],
            &[("prev", "Focus the previous trail entry")],
        ),
        HelpTopic::TrailNext => print_help_page(
            "halleyctl trail next",
            &["halleyctl trail next [-o OUTPUT]"],
            &[("next", "Focus the next trail entry")],
        ),
        HelpTopic::TrailList => print_help_page(
            "halleyctl trail list",
            &["halleyctl trail list [-o OUTPUT] [--json]"],
            &[(
                "list",
                "List trail entries on the focused or selected output",
            )],
        ),
        HelpTopic::TrailGoto => print_help_page(
            "halleyctl trail goto",
            &["halleyctl trail goto <TARGET> [-o OUTPUT]"],
            &[(
                "TARGET",
                "Use a trail index or the same selectors accepted by node commands",
            )],
        ),
        HelpTopic::Monitor => print_help_page(
            "halleyctl monitor",
            &["halleyctl monitor focus <left|right|up|down|OUTPUT>"],
            &[("focus", "Focus an adjacent monitor or a named output")],
        ),
        HelpTopic::MonitorFocus => print_help_page(
            "halleyctl monitor focus",
            &["halleyctl monitor focus <left|right|up|down|OUTPUT>"],
            &[(
                "left|right|up|down|OUTPUT",
                "Direction or exact output name to focus",
            )],
        ),
        HelpTopic::Bearings => print_help_page(
            "halleyctl bearings",
            &[
                "halleyctl bearings show",
                "halleyctl bearings hide",
                "halleyctl bearings toggle",
                "halleyctl bearings status",
            ],
            &[
                ("show", "Enable bearings"),
                ("hide", "Hide bearings"),
                ("toggle", "Toggle bearings visibility"),
                ("status", "Print current bearings visibility"),
            ],
        ),
        HelpTopic::BearingsShow => print_help_page(
            "halleyctl bearings show",
            &["halleyctl bearings show"],
            &[("show", "Enable bearings")],
        ),
        HelpTopic::BearingsHide => print_help_page(
            "halleyctl bearings hide",
            &["halleyctl bearings hide"],
            &[("hide", "Hide bearings")],
        ),
        HelpTopic::BearingsToggle => print_help_page(
            "halleyctl bearings toggle",
            &["halleyctl bearings toggle"],
            &[("toggle", "Toggle bearings visibility")],
        ),
        HelpTopic::BearingsStatus => print_help_page(
            "halleyctl bearings status",
            &["halleyctl bearings status"],
            &[("status", "Print current bearings visibility")],
        ),
        HelpTopic::Cluster => print_help_page(
            "halleyctl cluster",
            &[
                "halleyctl cluster list [-o OUTPUT] [--json]",
                "halleyctl cluster inspect [current|ID] [-o OUTPUT] [--json]",
                "halleyctl cluster layout cycle [-o OUTPUT]",
            ],
            &[
                ("list", "List clusters"),
                ("inspect", "Show information about a cluster"),
                ("layout", "Cluster workspace layout actions"),
            ],
        ),
        HelpTopic::ClusterList => print_help_page(
            "halleyctl cluster list",
            &["halleyctl cluster list [-o OUTPUT] [--json]"],
            &[("list", "List clusters on one or more outputs")],
        ),
        HelpTopic::ClusterInspect => print_help_page(
            "halleyctl cluster inspect",
            &["halleyctl cluster inspect [current|ID] [-o OUTPUT] [--json]"],
            &[(
                "current|ID",
                "Inspect the active cluster on an output or a cluster by id",
            )],
        ),
        HelpTopic::ClusterLayout => print_help_page(
            "halleyctl cluster layout",
            &["halleyctl cluster layout cycle [-o OUTPUT]"],
            &[("cycle", "Toggle the active cluster workspace layout")],
        ),
        HelpTopic::ClusterLayoutCycle => print_help_page(
            "halleyctl cluster layout cycle",
            &["halleyctl cluster layout cycle [-o OUTPUT]"],
            &[("cycle", "Toggle between tiling and stacking layouts")],
        ),
        HelpTopic::Stack => print_help_page(
            "halleyctl stack",
            &[
                "halleyctl stack cycle forward [-o OUTPUT]",
                "halleyctl stack cycle backward [-o OUTPUT]",
            ],
            &[(
                "cycle",
                "Rotate the active stacking deck forward or backward",
            )],
        ),
        HelpTopic::StackCycle => print_help_page(
            "halleyctl stack cycle",
            &[
                "halleyctl stack cycle forward [-o OUTPUT]",
                "halleyctl stack cycle backward [-o OUTPUT]",
            ],
            &[("forward|backward", "Cycle the active stacking deck")],
        ),
        HelpTopic::Tile => print_help_page(
            "halleyctl tile",
            &[
                "halleyctl tile focus left|right|up|down [-o OUTPUT]",
                "halleyctl tile swap left|right|up|down [-o OUTPUT]",
            ],
            &[
                ("focus", "Move focus to an adjacent visible tile"),
                (
                    "swap",
                    "Swap the focused tile with an adjacent visible tile",
                ),
            ],
        ),
        HelpTopic::TileFocus => print_help_page(
            "halleyctl tile focus",
            &["halleyctl tile focus left|right|up|down [-o OUTPUT]"],
            &[(
                "left|right|up|down",
                "Direction of the adjacent visible tile",
            )],
        ),
        HelpTopic::TileSwap => print_help_page(
            "halleyctl tile swap",
            &["halleyctl tile swap left|right|up|down [-o OUTPUT]"],
            &[(
                "left|right|up|down",
                "Direction of the adjacent visible tile",
            )],
        ),
    }
}

fn print_help_page(title: &str, usage: &[&str], commands: &[(&str, &str)]) {
    println!("{title}");
    println!();
    println!("Usage:");
    for line in usage {
        println!("  {line}");
    }
    if !commands.is_empty() {
        println!();
        println!("Commands:");
        for (name, description) in commands {
            println!("  {name:<9} {description}");
        }
    }
    println!();
    println!("Options:");
    println!("  -h, --help  Show help");
}
