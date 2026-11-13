use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use jarscope::find::{Query, find};
use jarscope::jar::{read_info, read_names, scan_jar};
use jarscope::render::{
    render_find, render_find_summary, render_info, render_list, render_version,
};

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
    #[command(about = "Show manifest fields, entry counts and sizes of a JAR")]
    Info {
        #[arg(help = "Path to the JAR file")]
        jar: PathBuf,
    },
    #[command(about = "Find which JARs in a folder contain a class")]
    Find {
        #[arg(
            help = "Class name (com.foo.Bar or com/foo/Bar) or pattern ('com.foo.*Service', '**.Logger')"
        )]
        class: String,
        #[arg(help = "Folder to search recursively (or a single JAR)")]
        dir: PathBuf,
        #[arg(
            long,
            help = "Also search JARs inside JARs (META-INF/jars, META-INF/jarjar, BOOT-INF/lib, WEB-INF/lib)"
        )]
        nested: bool,
    },
}

fn write_stdout(text: &str, success: ExitCode) -> ExitCode {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => success,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => success,
        Err(e) => {
            stderr_line(&format!("error: {e}"));
            ExitCode::from(2)
        }
    }
}

// eprintln! panics when stderr is closed; find can emit hundreds of lines.
fn stderr_line(line: &str) {
    let _ = writeln!(io::stderr().lock(), "{line}");
}

fn fail(err: &anyhow::Error) -> ExitCode {
    stderr_line(&format!("error: {err:#}"));
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { jar } => match scan_jar(&jar) {
            Ok(report) => {
                if report.duplicate_names {
                    stderr_line(&format!(
                        "warning: {} has duplicate entry names; classes are counted once per name",
                        jar.display()
                    ));
                }
                write_stdout(&render_version(&report), ExitCode::SUCCESS)
            }
            Err(err) => fail(&err),
        },
        Command::List { jar, classes_only } => match read_names(&jar) {
            Ok(names) => write_stdout(&render_list(&names, classes_only), ExitCode::SUCCESS),
            Err(err) => fail(&err),
        },
        Command::Info { jar } => match read_info(&jar) {
            Ok(info) => write_stdout(&render_info(&info), ExitCode::SUCCESS),
            Err(err) => fail(&err),
        },
        Command::Find { class, dir, nested } => {
            let query = match Query::parse(&class) {
                Ok(q) => q,
                Err(err) => return fail(&err),
            };
            let start = Instant::now();
            let report = match find(&dir, &query, nested) {
                Ok(r) => r,
                Err(err) => return fail(&err),
            };
            let elapsed = start.elapsed();
            for w in &report.warnings {
                stderr_line(&format!("warning: {w}"));
            }
            let code = if report.matches.is_empty() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            };
            let code = write_stdout(&render_find(&report, &query), code);
            let summary = render_find_summary(&report, elapsed);
            let _ = write!(io::stderr().lock(), "{summary}");
            code
        }
    }
}
