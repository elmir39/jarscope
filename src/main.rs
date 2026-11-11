use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use jarscope::jar::{read_index, scan_jar};
use jarscope::render::{render_list, render_version};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Show the Java version a JAR requires, with a per-version class breakdown")]
    Version {
        #[arg(help = "Path to the JAR file")]
        jar: PathBuf,
    },
    #[command(about = "List the entries of a JAR, one per line, grouped by package")]
    List {
        #[arg(help = "Path to the JAR file")]
        jar: PathBuf,
        #[arg(long, help = "List only classes, as dotted names")]
        classes_only: bool,
    },
}

fn write_stdout(text: &str) -> ExitCode {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { jar } => match scan_jar(&jar) {
            Ok(report) => write_stdout(&render_version(&report)),
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            }
        },
        Command::List { jar, classes_only } => match read_index(&jar) {
            Ok(report) => {
                if report.duplicate_names {
                    eprintln!(
                        "warning: {} has duplicate entry names; each name is listed once",
                        jar.display()
                    );
                }
                for (name, reason) in &report.invalid {
                    eprintln!("warning: {name}: {reason}");
                }
                write_stdout(&render_list(&report, classes_only))
            }
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::from(2)
            }
        },
    }
}
