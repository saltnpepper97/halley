use halley_ipc::{
    LogicalOutputInfo, NodeMoveDirection, OutputInfo, OutputStatus, OutputsResponse, Request,
    Response, send_request,
};

fn main() {
    let mut args = std::env::args().skip(1);

    let request = match args.next().as_deref() {
        Some("quit") => Request::Quit,
        Some("reload") => Request::Reload,
        Some("outputs") => Request::Outputs,
        Some("docking") => match args.next().as_deref() {
            Some("begin") => Request::DockingBegin,
            Some("end") => Request::DockingEnd,
            _ => {
                eprintln!("usage: halleyctl docking begin|end");
                std::process::exit(2);
            }
        },
        Some("node") => match args.next().as_deref() {
            Some("move") => match args.next().as_deref() {
                Some("left") => Request::NodeMove(NodeMoveDirection::Left),
                Some("right") => Request::NodeMove(NodeMoveDirection::Right),
                Some("up") => Request::NodeMove(NodeMoveDirection::Up),
                Some("down") => Request::NodeMove(NodeMoveDirection::Down),
                _ => {
                    eprintln!("usage: halleyctl node move left|right|up|down");
                    std::process::exit(2);
                }
            },
            _ => {
                eprintln!("usage: halleyctl node move left|right|up|down");
                std::process::exit(2);
            }
        },
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
            return;
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    };

    match send_request(&request) {
        Ok(response) => {
            if let Err(err) = print_response(response) {
                eprintln!("halleyctl failed: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("halleyctl failed to talk to halley: {err}");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!("halleyctl");
    println!();
    println!("Usage:");
    println!("  halleyctl quit");
    println!("  halleyctl reload");
    println!("  halleyctl outputs");
    println!("  halleyctl docking begin");
    println!("  halleyctl docking end");
    println!("  halleyctl node move left");
    println!("  halleyctl node move right");
    println!("  halleyctl node move up");
    println!("  halleyctl node move down");
    println!();
    println!("Commands:");
    println!("  quit      Ask the running Halley compositor to exit");
    println!("  reload    Ask the running Halley compositor to reload config");
    println!("  outputs   Print current output information from the running Halley compositor");
    println!("  docking   Control compositor docking state");
    println!("  node      Run node-scoped commands");
}

fn print_response(response: Response) -> Result<(), String> {
    match response {
        Response::Ok => {
            println!("ok");
            Ok(())
        }
        Response::Reloaded => {
            println!("reloaded");
            Ok(())
        }
        Response::Outputs(outputs) => {
            print_outputs(outputs);
            Ok(())
        }
        Response::Error(err) => Err(format!("{err:?}")),
    }
}

fn print_outputs(outputs: OutputsResponse) {
    if outputs.outputs.is_empty() {
        println!("No outputs.");
        return;
    }

    for output in outputs.outputs {
        print_output(&output);
    }
}

fn print_output(output: &OutputInfo) {
    println!("{}", output.name);
    println!("  status: {}", format_status(output.status));
    println!(
        "  enabled: {}",
        if output.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    if let Some(current_mode) = &output.current_mode {
        println!("  current_mode: {}", current_mode.display_string());
    }

    if output.modes.is_empty() {
        println!("  modes: (none)");
    } else {
        let has_refresh = output.modes.iter().any(|mode| mode.refresh_hz.is_some());
        if has_refresh {
            println!("  modes:");
        } else {
            println!("  modes: (resolution-only; refresh unavailable)");
        }

        for mode in &output.modes {
            let marker = if mode.current {
                "*"
            } else if mode.preferred {
                "+"
            } else {
                "-"
            };
            println!("    {marker} {}", mode.display_string());
        }
    }

    if let Some(logical) = &output.logical {
        print_logical(logical);
    }
}

fn print_logical(logical: &LogicalOutputInfo) {
    println!("  logical:");
    println!("    scale: {}", logical.scale);
    println!("    focused: {}", logical.focused);
    println!("    offset: {}, {}", logical.offset_x, logical.offset_y);
}

fn format_status(status: OutputStatus) -> &'static str {
    match status {
        OutputStatus::Connected => "connected",
        OutputStatus::Disconnected => "disconnected",
        OutputStatus::Unknown => "unknown",
    }
}
