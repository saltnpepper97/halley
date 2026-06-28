#![allow(
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

mod standalone;

fn main() {
    if let Err(err) = standalone::run() {
        eprintln!("halley-aperture exited with error: {err}");
        std::process::exit(1);
    }
}
