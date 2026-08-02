//! Shannon-entropy detection (spec §7.5(b)): "strings ≥20 chars, Shannon
//! entropy >4.2 bits/char, not matching an ID/hash allowlist (our own
//! blake3 IDs, git SHAs, content sha256s are exempt by field, not by
//! pattern)". The "exempt by field" half of that sentence is satisfied
//! structurally here — `redact::mod` only ever calls this over
//! `TaintedString` fields (currently just `SymbolNode.signature`), never
//! over `NodeId`/`sha256` fields, which aren't `TaintedString` and never
//! reach this module at all.
//!
//! Scans per-*token*, not over the whole input string: entropy computed
//! across a mixed-punctuation sentence would be diluted below any
//! meaningful threshold by ordinary low-entropy separators (spaces,
//! parens, commas), and would need windowing to find the actual
//! high-entropy substring anyway. Splitting on token boundaries first
//! and scoring each token's own entropy is the standard approach real
//! secret scanners (gitleaks, detect-secrets) use — see ADR-0017.

use super::patterns::Match;

const MIN_LEN: usize = 20;
const MIN_ENTROPY_BITS_PER_CHAR: f64 = 4.2;

/// Token alphabet: alnum plus base64/hex "value" punctuation (`+`, `/`,
/// `=`, `-`). Deliberately excludes `.` — dotted identifiers/paths/
/// version strings are common and would otherwise glue unrelated
/// low-entropy words into one long, spuriously-scored token.
/// Deliberately excludes `_` too, found by dogfooding this pass against
/// carto's own source (a long, descriptive `snake_case_test_name` —
/// this codebase's own convention — merged a dozen lexically-distinct
/// English words into one 76-char token via its underscores, and their
/// combined lexical diversity alone pushed it over the entropy
/// threshold). A real base64/hex secret blob essentially never contains
/// a literal underscore as a value character (standard base64 doesn't
/// use one at all; base64url's `_` is already caught separately by
/// `patterns::find_jwt`'s explicit three-segment shape) — false
/// positives on ordinary `snake_case` identifiers are far more likely
/// in practice than a false negative on an unprefixed, underscore-
/// bearing secret with no other distinguishing shape. See ADR-0017.
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-')
}

/// Finds every token ≥[`MIN_LEN`] chars whose Shannon entropy exceeds
/// [`MIN_ENTROPY_BITS_PER_CHAR`] and whose byte range doesn't overlap
/// any span in `already_matched` (a pattern match takes priority — an
/// AWS key is reported once, under `"aws_access_key"`, not twice).
pub fn find_high_entropy_tokens(text: &str, already_matched: &[Match]) -> Vec<Match> {
    let mut out = Vec::new();
    for (start, end) in token_byte_ranges(text) {
        if end - start < MIN_LEN {
            continue;
        }
        if already_matched
            .iter()
            .any(|(r, _)| r.start < end && start < r.end)
        {
            continue;
        }
        let token = &text[start..end];
        if shannon_entropy_bits_per_char(token) > MIN_ENTROPY_BITS_PER_CHAR {
            out.push((start..end, "high_entropy"));
        }
    }
    out
}

/// Byte `(start, end)` ranges of maximal runs of [`is_token_char`]
/// characters in `text`.
fn token_byte_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut run_start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if is_token_char(c) {
            run_start.get_or_insert(i);
        } else if let Some(s) = run_start.take() {
            ranges.push((s, i));
        }
    }
    if let Some(s) = run_start {
        ranges.push((s, text.len()));
    }
    ranges
}

/// Shannon entropy of `s`'s character distribution, in bits per
/// character: `-Σ p(c) log2 p(c)`.
fn shannon_entropy_bits_per_char(s: &str) -> f64 {
    let mut counts: std::collections::BTreeMap<char, u32> = std::collections::BTreeMap::new();
    let mut total = 0u32;
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
        total += 1;
    }
    if total == 0 {
        return 0.0;
    }
    let total_f = total as f64;
    -counts
        .values()
        .map(|&n| {
            let p = n as f64 / total_f;
            p * p.log2()
        })
        .sum::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn categories(text: &str) -> Vec<&'static str> {
        find_high_entropy_tokens(text, &[])
            .into_iter()
            .map(|(_, c)| c)
            .collect()
    }

    #[test]
    fn high_entropy_random_looking_token_is_flagged() {
        // A random-looking 32-char mixed-case+digit token, entropy well
        // above 4.2 bits/char over a ~62-symbol alphabet.
        assert_eq!(
            categories("token = \"xJ4kLpQ9rT2vN8mF6wZ1yB3cH5dS7aE\""),
            vec!["high_entropy"]
        );
    }

    #[test]
    fn short_token_is_not_flagged_even_if_dense() {
        assert!(categories("id = \"xJ4kLpQ9rT2v\"").is_empty()); // 12 chars, < MIN_LEN
    }

    #[test]
    fn git_sha_is_not_flagged_hex_entropy_is_below_threshold() {
        // 40 hex chars: max possible entropy is log2(16) = 4.0 bits/char,
        // which is < 4.2 regardless of "randomness" -- hex alphabet alone
        // rules this out, exactly the "git SHAs are exempt" case spec
        // §7.5(b) names, achieved here by alphabet math, not an allowlist.
        assert!(categories("commit_sha = \"a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\"").is_empty());
    }

    #[test]
    fn low_entropy_repeated_text_is_not_flagged() {
        assert!(categories("name = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"").is_empty());
    }

    #[test]
    fn ordinary_english_sentence_is_not_flagged() {
        assert!(
            categories("fn calculate_total_price_including_tax(order: &Order) -> f64").is_empty()
        );
    }

    #[test]
    fn long_descriptive_snake_case_test_name_is_not_flagged() {
        // Regression: found by dogfooding this exact pass against
        // carto's own source (see ADR-0017) -- a long, descriptive
        // snake_case test function name (this codebase's own testing
        // convention, CLAUDE.md) used to merge a dozen lexically-
        // distinct English words into one long token via its
        // underscores, and their combined lexical diversity alone
        // pushed it over the 4.2 bits/char threshold. `_` is no longer
        // in the token alphabet, so this now splits into a dozen short
        // words, none reaching MIN_LEN.
        assert!(
            categories(
                "fn csharp_namespace_import_with_unknown_root_keys_external_module_by_full_string()"
            )
            .is_empty()
        );
    }

    #[test]
    fn already_matched_span_is_not_reported_twice() {
        let text = "AKIAIOSFODNN7EXAMPLE";
        let pattern_match = vec![(0..text.len(), "aws_access_key")];
        assert!(find_high_entropy_tokens(text, &pattern_match).is_empty());
    }
}
