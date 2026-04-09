use std::time::{Duration, Instant};

use halley_ipc::{CaptureRequest, Request, Response, send_request};

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
                let original_request = request.clone();
                if let Err(err) = print_response(response.clone()) {
                    eprintln!("halleyctl failed: {err}");
                    std::process::exit(1);
                }
                if let Request::Capture(CaptureRequest::Start { .. }) = original_request {
                    let Some(serial) = capture_serial_from_response(&response) else {
                        return;
                    };
                    if let Err(err) = wait_for_capture_result(serial) {
                        eprintln!("halleyctl failed: {err}");
                        std::process::exit(1);
                    }
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

fn capture_serial_from_response(response: &Response) -> Option<u64> {
    match response {
        Response::CaptureStatus(status) => status.session_serial,
        _ => None,
    }
}

fn wait_for_capture_result(serial: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if Instant::now() > deadline {
            return Err("timed out waiting for capture result".to_string());
        }
        let response = send_request(&Request::Capture(CaptureRequest::Status))
            .map_err(|err| format!("failed to query capture status: {err}"))?;
        let Response::CaptureStatus(status) = response else {
            return Err("unexpected response while waiting for capture result".to_string());
        };
        if status.last_finished_serial == Some(serial) {
            if let Some(path) = status.saved_path {
                println!("saved: {path}");
                return Ok(());
            }
            if let Some(error) = status.error {
                return Err(error);
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}
