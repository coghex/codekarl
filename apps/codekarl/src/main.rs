//! The `codekarl` executable: a thin `main` over the `codekarl` library, which holds the program and its
//! documentation. Only this file reads the process arguments.

use std::process::ExitCode;

fn main() -> ExitCode {
    codekarl::main_entry(std::env::args_os().skip(1).collect())
}
