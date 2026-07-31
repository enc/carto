use carto_core::taint::{Provenance, TaintedString};

fn main() {
    let t = TaintedString::new("hello", Provenance::Syntactic);
    let _s: &str = t.as_ref();
}
