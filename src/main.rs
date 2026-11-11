use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jarscope::jar::scan_jar;
use jarscope::render::render_version;

/// Inspects Java JAR files without a JVM.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the Java version a JAR requires, with a per-version class breakdown
    Version {
        /// Path to the JAR file
        jar: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { jar } => match scan_jar(&jar) {
            Ok(report) => {
                let text = render_version(&report);
                match io::stdout().lock().write_all(text.as_bytes()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::from(2)
                    }
                }
            }
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            }
        },
    }
}
