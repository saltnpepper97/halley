fn main() {
    let session = std::env::args_os().skip(1).any(|arg| arg == "--session");
    let result = if session {
        halley_wl::run_session()
    } else {
        halley_wl::run()
    };

    if let Err(err) = result {
        eprintln!("halley-wl exited with error: {err}");
        std::process::exit(1);
    }
}
