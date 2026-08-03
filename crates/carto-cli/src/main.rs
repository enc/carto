//! carto: bin target for the `carto` CLI. clap root, exit-code mapping
//! (spec §9.2). `selfcheck` (M1.a), `index` (M1.b.1),
//! `where`/`deps`/`map` (M1.b.3), and `serve` (the MCP stdio server,
//! spec §7.3, mounted from `carto-mcp`) exist.

mod deps_cmd;
mod index;
mod map_cmd;
mod selfcheck;
mod where_cmd;

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
    /// Build the structural graph of a repo (spec §7.1).
    Index(index::IndexArgs),
    /// Find symbols by name (spec §7.1). Module name is `where_cmd`
    /// (`where` is a Rust keyword) — the subcommand itself is `where`.
    #[command(name = "where")]
    Where(where_cmd::WhereArgs),
    /// Adjacency listing with confidence (spec §7.1).
    Deps(deps_cmd::DepsArgs),
    /// Layered overview, hard-capped at --budget lines (spec §7.1).
    Map(map_cmd::MapArgs),
    /// Run the MCP stdio server (spec §7.3): newline-delimited JSON-RPC
    /// on stdin/stdout, exposing `index`/`where`/`deps`/`map`/`selfcheck`
    /// as tools — the same core functions this CLI's own subcommands
    /// call. Runs until stdin closes (EOF). `--json` has no effect here;
    /// the wire format is always JSON-RPC, never this CLI's human/--json
    /// output split.
    Serve,
}

fn main() {
    let cli = Cli::parse();
    let exit_code = run(&cli).unwrap_or_else(|e| {
        // Logs to stderr only — stdout is data (spec §9.2).
        eprintln!("carto: {e}");
        e.exit_code()
    });
    std::process::exit(exit_code.into());
}

/// Dispatches to each command's implementation and emits its output
/// (spec §9.2: `--json` serializes the same struct the human renderer
/// reads from; stdout carries only one or the other, diagnostics go to
/// stderr). A command's own failure (user/data/invariant) propagates as
/// `Err` and is mapped to the process exit code in exactly one place
/// ([`main`]), rather than each command wiring up its own exit code.
fn run(cli: &Cli) -> carto_core::Result<u8> {
    match &cli.command {
        Command::Selfcheck => {
            let report = selfcheck::run();
            emit(cli.json, &report, || selfcheck::print_human(&report))?;
        }
        Command::Index(args) => {
            let summary = index::run(args)?;
            emit(cli.json, &summary, || index::print_human(&summary))?;
        }
        Command::Where(args) => {
            let result = where_cmd::run(args)?;
            emit(cli.json, &result, || where_cmd::print_human(&result))?;
        }
        Command::Deps(args) => {
            let result = deps_cmd::run(args)?;
            emit(cli.json, &result, || {
                deps_cmd::print_human(&result, args.dir.into())
            })?;
        }
        Command::Map(args) => {
            let result = map_cmd::run(args)?;
            emit(cli.json, &result, || map_cmd::print_human(&result))?;
        }
        Command::Serve => {
            // Own protocol, own framing — bypasses `emit`'s human/--json
            // split entirely; the JSON-RPC wire format on stdout *is* the
            // output for the whole lifetime of this command, not one
            // summary printed at the end.
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            carto_mcp::serve(stdin.lock(), stdout.lock())?;
        }
    }
    Ok(0)
}

/// `--json`: serialize `value` to stdout. Otherwise, call `human`, which
/// prints the same information in human form. Never both.
fn emit<T: serde::Serialize>(
    json: bool,
    value: &T,
    human: impl FnOnce(),
) -> carto_core::Result<()> {
    if json {
        let s = serde_json::to_string(value).map_err(|e| {
            carto_core::Error::with_source(
                carto_core::ErrorKind::DataError,
                "failed to serialize output",
                e,
            )
        })?;
        println!("{s}");
    } else {
        human();
    }
    Ok(())
}
