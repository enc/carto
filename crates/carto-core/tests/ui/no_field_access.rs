use carto_core::taint::{Provenance, TaintedString};

fn main() {
    let t = TaintedString::new("hello", Provenance::Syntactic);
    // `sanitized` is a private field of TaintedString; this must not compile
    // from outside the `taint` module.
    let _s: &str = &t.sanitized;
}
