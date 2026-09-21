//! `carto selfcheck` — spec §7.1: "environment report: versions, grammar
//! mode, landlock status".
//!
//! This module is the *only* place INV-2 permits `std::process::Command`
//! (see clippy.toml's `disallowed-methods` exemption), and even so, M1.a
//! does not use it yet — there is nothing to shell out to until grammar
//! mode / rustc reporting needs it. The exemption exists now so a future
//! addition here doesn't need a clippy.toml change reviewed under time
//! pressure.

use carto_core::consts::{BIN_NAME, SCHEMA_VERSION};
use carto_core::{outdir, pathguard};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct SelfcheckReport {
    pub carto_version: &'static str,
    pub schema_version: u32,
    pub git_sha: &'static str,
    /// The MSRV declared in `Cargo.toml` (`rust-version`), not the exact
    /// compiler used to build this binary — reporting the latter would
    /// need a subprocess call in build.rs, which ADR-0003 rules out.
    pub min_rust_version: &'static str,
    pub target_triple: &'static str,
    /// Hardcoded for now: M1–M4 build native grammars behind the
    /// `native-grammars` feature (spec §5.4). `wasm-grammars` becomes
    /// default in M5.
    pub grammar_mode: &'static str,
    /// Spec §14: "Windows: no Landlock" / non-Linux in general —
    /// `selfcheck` reports "confinement: none (platform)" and does not
    /// block release. Landlock itself lands in M5.
    pub confinement: &'static str,
    pub default_out_root: Option<PathBuf>,
    /// Digest of the INV-4 hard denylist (spec §7.4), so an operator can
    /// confirm which denylist a given carto build is enforcing.
    pub pathguard_denylist_digest: String,
}

pub fn run() -> SelfcheckReport {
    let confinement = if cfg!(target_os = "linux") {
        "none (landlock not yet implemented — lands in M5)"
    } else {
        "none (platform)"
    };

    let default_out_root = std::env::current_dir()
        .ok()
        .and_then(|cwd| outdir::default_out_root(&cwd).ok());

    SelfcheckReport {
        carto_version: env!("CARGO_PKG_VERSION"),
        schema_version: SCHEMA_VERSION,
        git_sha: env!("CARTO_GIT_SHA"),
        min_rust_version: env!("CARGO_PKG_RUST_VERSION"),
        target_triple: env!("CARTO_TARGET_TRIPLE"),
        grammar_mode: "native (rust, python, php, typescript, tsx, javascript, go, csharp, hcl)",
        confinement,
        default_out_root,
        pathguard_denylist_digest: pathguard::denylist_digest(),
    }
}

pub fn print_human(report: &SelfcheckReport) {
    println!("{BIN_NAME} {}", report.carto_version);
    println!("  schema_version:  {}", report.schema_version);
    println!("  git_sha:         {}", report.git_sha);
    println!("  min_rust_version: {}", report.min_rust_version);
    println!("  target:          {}", report.target_triple);
    println!("  grammar_mode:    {}", report.grammar_mode);
    println!("  confinement:     {}", report.confinement);
    match &report.default_out_root {
        Some(p) => println!("  default out-dir: {}", p.display()),
        None => println!("  default out-dir: <unresolvable: cwd not accessible>"),
    }
    println!("  denylist digest: {}", report.pathguard_denylist_digest);
}
