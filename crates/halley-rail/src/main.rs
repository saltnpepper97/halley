mod standalone;

fn main() {
    if let Err(err) = standalone::run() {
        eprintln!("halley-rail exited with error: {err}");
        std::process::exit(1);
    }
}
