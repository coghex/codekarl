//! Headless scenario-driven economy production and payday soak.

use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match codekarl::economy_soak_main(env::args().skip(1)) {
        Ok(csv) => {
            print!("{csv}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("economy-soak: {message}");
            ExitCode::FAILURE
        }
    }
}
