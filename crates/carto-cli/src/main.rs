//! carto: bin target for the `carto` CLI. clap root, exit-code mapping
//! (spec §9.2). No `index`/`where`/`deps`/`map` yet — those arrive with
//! their implementations in M1.b; only `selfcheck` exists in M1.a.

mod selfcheck;

use clap::{Parser, Subcommand};

/// carto — infra-aware code map for coding agents.
#[derive(Parser)]
#[command(name = carto_core::consts::BIN_NAME, version)]
struct Cli {
    /// Emit machine-readable JSON on stdout instead of human text.
    /// carto-wide convention (spec §9.2): stdout is always data, logs
    /// always go to stderr, regardless of this flag.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Environment report: versions, grammar mode, landlock status.
    Selfcheck,
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match &cli.command {
        Command::Selfcheck => run_selfcheck(cli.json),
    };

    std::process::exit(exit_code.into());
}

fn run_selfcheck(json: bool) -> u8 {
    let report = selfcheck::run();
    if json {
        match serde_json::to_string(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                // Logs to stderr only — stdout is data (spec §9.2).
                eprintln!("carto: failed to serialize selfcheck report: {e}");
                return carto_core::ErrorKind::DataError.exit_code();
            }
        }
    } else {
        selfcheck::print_human(&report);
    }
    0
}
