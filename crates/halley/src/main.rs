fn main() {
    if let Err(err) = halley_wl::run() {
        eprintln!("halley-wl exited with error: {err}");
        std::process::exit(1);
    }
}
