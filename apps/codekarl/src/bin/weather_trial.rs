//! Headless weather trial.
use std::{env, process::ExitCode};
fn main() -> ExitCode {
    match codekarl::weather_trial_main(env::args().skip(1)) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("weather-trial: {message}");
            ExitCode::FAILURE
        }
    }
}
