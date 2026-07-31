use carto_core::taint::{Provenance, TaintedString};

fn main() {
    let t = TaintedString::new("hello", Provenance::Syntactic);
    let _s: String = t.into();
}
