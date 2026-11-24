//! Headless itinerary and travel-time report.

use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match codekarl::travel_times_main(env::args().skip(1)) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("travel-times: {message}");
            ExitCode::FAILURE
        }
    }
}
