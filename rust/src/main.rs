use qexow_cam::cli;
use std::io::{self, ErrorKind, Write};

fn main() {
    match cli::execute_from_env() {
        Ok(output) => {
            if !output.is_empty() {
                if let Err(error) = writeln!(io::stdout(), "{output}") {
                    if error.kind() != ErrorKind::BrokenPipe {
                        eprintln!("cam error: failed to write output: {error}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Err(error) => {
            eprintln!("cam error: {error}");
            std::process::exit(1);
        }
    }
}
