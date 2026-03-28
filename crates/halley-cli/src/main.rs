use halley_ipc::send_request;

mod cmd;
mod help;
mod parse;
mod print;

use help::{exit_usage, print_help};
use parse::{ParseOutcome, parse_request};
use print::print_response;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_request(&args) {
        Ok(ParseOutcome::Request(request)) => match send_request(&request) {
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
        },
        Ok(ParseOutcome::Help(topic)) => print_help(topic),
        Err(err) => exit_usage(err),
    }
}
