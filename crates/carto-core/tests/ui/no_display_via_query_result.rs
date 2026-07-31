// The query layer (crate::query) is the first place a TaintedString
// travels all the way out to a CLI/MCP renderer (SymbolMatch.signature,
// spec §7.1's `where` output). This is the same underlying failure as
// no_display.rs, asserted at that call site specifically: a query
// result's signature field must not be println!-able directly, since
// this is exactly the call site a future renderer change might get
// wrong.
use carto_core::graph::{SymKind, file_id};
use carto_core::query::SymbolMatch;
use carto_core::taint::{Provenance, TaintedString};

fn main() {
    let m = SymbolMatch {
        id: file_id("src/lib.rs"),
        name: "foo".to_string(),
        sym_kind: SymKind::Function,
        location: "src/lib.rs:1-2".to_string(),
        signature: Some(TaintedString::new("fn foo()", Provenance::Syntactic)),
        provenance: Provenance::Syntactic,
    };
    if let Some(sig) = &m.signature {
        println!("{}", sig);
    }
}
