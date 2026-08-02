//! Hand-rolled pattern matchers (spec §7.5(a)) — no `regex` dependency.
//! Every pattern here is a fixed prefix or a fixed structural shape
//! (a prefix + a run of chars from a known alphabet, or a literal
//! substring), all cheap to scan by hand over the short (line-sized)
//! strings this module ever sees (`TaintedString::render_capped`
//! output — a symbol's signature, never a whole file). Consistent with
//! this codebase's general preference for hand-rolled scanning over a
//! new dependency when the win is marginal (ADR-0005 "no petgraph",
//! ADR-0015 "no go.mod parsing") — see
//! `docs/adr/0017-redaction-scope-and-detection-design.md`.
//!
//! Each `find_*` function returns non-overlapping `(Range<usize>,
//! category)` matches in left-to-right order. `redact::mod` merges
//! these across all patterns (first-match-wins on overlap) before
//! running the entropy pass.

use std::ops::Range;

/// One matched span plus which category it belongs to — the category
/// string is also the `RedactionCounts.by_category` key.
pub type Match = (Range<usize>, &'static str);

/// Runs every pattern matcher over `text`, returning all matches in
/// left-to-right, non-overlapping order (a pattern that would overlap
/// an earlier one — not expected given how distinct these prefixes are,
/// but not assumed — is dropped, first-found-wins, same house style as
/// `resolve.rs`'s ambiguity handling).
pub fn find_all(text: &str) -> Vec<Match> {
    let mut matches = Vec::new();
    for finder in [
        find_aws_access_key as fn(&str) -> Vec<Match>,
        find_secret_key_near_keyword,
        find_private_key_pem,
        find_github_token,
        find_gitlab_token,
        find_slack_token,
        find_jwt,
        find_connection_string,
    ] {
        for m in finder(text) {
            if !matches.iter().any(|(r, _): &Match| overlaps(r, &m.0)) {
                matches.push(m);
            }
        }
    }
    matches.sort_by_key(|(r, _)| r.start);
    matches
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// `A3T`/`AKIA`/`ASIA` + exactly 16 uppercase-alnum chars (spec §7.5(a):
/// `(A3T|AKIA|ASIA)[A-Z0-9]{16}`).
fn find_aws_access_key(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for prefix in ["A3T", "AKIA", "ASIA"] {
        let mut start = 0;
        while let Some(rel) = find_from(text, prefix, start) {
            let tail_start = rel + prefix.len();
            let tail_end = take_while(bytes, tail_start, |c| {
                c.is_ascii_uppercase() || c.is_ascii_digit()
            });
            if tail_end - tail_start == 16 {
                out.push((rel..tail_end, "aws_access_key"));
                start = tail_end;
            } else {
                start = rel + 1;
            }
        }
    }
    out
}

/// A 40-char run of base64-alphabet characters co-occurring with the
/// word "secret" (case-insensitive) *anywhere in the same field* — spec
/// §7.5(a)'s "near". Fields scanned by this module are line-sized (a
/// symbol signature), so whole-field co-occurrence is the sane reading
/// of "near" rather than a byte-distance window; see ADR-0017. AWS
/// secret access keys are exactly 40 base64 chars, hence the exact
/// length rather than a minimum.
fn find_secret_key_near_keyword(text: &str) -> Vec<Match> {
    if !contains_ignore_case(text, "secret") {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_base64_char(bytes[i]) {
            let end = take_while(bytes, i, is_base64_char);
            if end - i == 40 {
                out.push((i..end, "secret_key_near_keyword"));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// `-----BEGIN [RSA|EC|DSA|OPENSSH ]PRIVATE KEY-----` (spec §7.5(a)).
fn find_private_key_pem(text: &str) -> Vec<Match> {
    let mut out = Vec::new();
    for header in [
        "-----BEGIN PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----",
        "-----BEGIN DSA PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----",
    ] {
        let mut start = 0;
        while let Some(rel) = find_from(text, header, start) {
            out.push((rel..rel + header.len(), "private_key_pem"));
            start = rel + header.len();
        }
    }
    out
}

/// GitHub token prefixes (`ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`) + a run of
/// alnum chars, minimum length chosen to comfortably clear GitHub's own
/// (36-char suffix) format while tolerating shorter fixture/test values.
fn find_github_token(text: &str) -> Vec<Match> {
    find_prefixed_token(
        text,
        &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        20,
        "github_token",
    )
}

/// GitLab personal access token prefix `glpat-` + alnum/`-`/`_` run.
fn find_gitlab_token(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let prefix = "glpat-";
    let mut start = 0;
    while let Some(rel) = find_from(text, prefix, start) {
        let tail_start = rel + prefix.len();
        let tail_end = take_while(bytes, tail_start, |c| {
            c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
        });
        if tail_end - tail_start >= 20 {
            out.push((rel..tail_end, "gitlab_token"));
            start = tail_end;
        } else {
            start = rel + 1;
        }
    }
    out
}

/// Slack token prefixes (`xoxb-`/`xoxp-`/`xoxa-`/`xoxr-`/`xoxs-`) + a
/// run of alnum/`-` chars.
fn find_slack_token(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
        let mut start = 0;
        while let Some(rel) = find_from(text, prefix, start) {
            let tail_start = rel + prefix.len();
            let tail_end = take_while(bytes, tail_start, |c| {
                c.is_ascii_alphanumeric() || c == b'-'
            });
            if tail_end - tail_start >= 10 {
                out.push((rel..tail_end, "slack_token"));
                start = tail_end;
            } else {
                start = rel + 1;
            }
        }
    }
    out
}

/// A JWT: `eyJ` + base64url run + `.` + base64url run + `.` + base64url
/// run (spec §7.5(a): "JWTs (`eyJ` base64 triplets)"). `eyJ` is the
/// near-universal JWT header prefix (`{"` base64url-encodes to exactly
/// that).
fn find_jwt(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(rel) = find_from(text, "eyJ", start) {
        let seg1_end = take_while(bytes, rel, is_base64url_char);
        if seg1_end < bytes.len() && bytes[seg1_end] == b'.' {
            let seg2_start = seg1_end + 1;
            let seg2_end = take_while(bytes, seg2_start, is_base64url_char);
            if seg2_end > seg2_start && seg2_end < bytes.len() && bytes[seg2_end] == b'.' {
                let seg3_start = seg2_end + 1;
                let seg3_end = take_while(bytes, seg3_start, is_base64url_char);
                if seg3_end > seg3_start {
                    out.push((rel..seg3_end, "jwt"));
                    start = seg3_end;
                    continue;
                }
            }
        }
        start = rel + 1;
    }
    out
}

/// `scheme://user:pass@host` — a credential-bearing connection string
/// (spec §7.5(a)). Deliberately narrow: requires a `:` and `@` both
/// appearing before the next `/` or whitespace after `://`, which is
/// exactly the user:pass@ shape and excludes a bare `https://host/path`.
fn find_connection_string(text: &str) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    while let Some(scheme_sep) = find_from(text, "://", start) {
        // Walk the scheme back to its own start (alnum/+/./-).
        let mut scheme_start = scheme_sep;
        while scheme_start > 0 {
            let c = bytes[scheme_start - 1];
            if c.is_ascii_alphanumeric() || c == b'+' || c == b'.' || c == b'-' {
                scheme_start -= 1;
            } else {
                break;
            }
        }
        if scheme_start == scheme_sep {
            start = scheme_sep + 3;
            continue;
        }
        let cred_start = scheme_sep + 3;
        let cred_end = take_while(bytes, cred_start, |c| {
            c != b'/' && c != b'@' && !c.is_ascii_whitespace() && c != b':'
        });
        let has_pass = cred_end < bytes.len() && bytes[cred_end] == b':';
        let pass_end = if has_pass {
            take_while(bytes, cred_end + 1, |c| {
                c != b'/' && c != b'@' && !c.is_ascii_whitespace()
            })
        } else {
            cred_end
        };
        let has_creds = cred_end > cred_start && has_pass && pass_end > cred_end + 1;
        if has_creds && pass_end < bytes.len() && bytes[pass_end] == b'@' {
            let host_end = take_while(bytes, pass_end + 1, |c| {
                c != b'/' && !c.is_ascii_whitespace()
            });
            out.push((scheme_start..host_end, "connection_string"));
            start = host_end;
        } else {
            start = scheme_sep + 3;
        }
    }
    out
}

/// Finds a token with one of `prefixes`, followed by at least
/// `min_tail_len` alnum/`_` chars.
fn find_prefixed_token(
    text: &str,
    prefixes: &[&str],
    min_tail_len: usize,
    category: &'static str,
) -> Vec<Match> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for &prefix in prefixes {
        let mut start = 0;
        while let Some(rel) = find_from(text, prefix, start) {
            let tail_start = rel + prefix.len();
            let tail_end = take_while(bytes, tail_start, |c| {
                c.is_ascii_alphanumeric() || c == b'_'
            });
            if tail_end - tail_start >= min_tail_len {
                out.push((rel..tail_end, category));
                start = tail_end;
            } else {
                start = rel + 1;
            }
        }
    }
    out
}

/// Byte offset of the next occurrence of `needle` in `text` at or after
/// byte offset `from`, or `None`. `text[from..]` is always a valid
/// slice boundary here since every caller advances `from` to a
/// previously-computed match end (itself always a char boundary — see
/// `take_while`), never into the middle of a multi-byte character.
fn find_from(text: &str, needle: &str, from: usize) -> Option<usize> {
    text.get(from..)?.find(needle).map(|i| i + from)
}

/// Extends `start` forward over consecutive bytes satisfying `pred`,
/// returning the end offset. All alphabets this module scans (base64,
/// base64url, hex, alnum, token-punctuation) are pure ASCII, so
/// byte-at-a-time scanning never splits a multi-byte UTF-8 character —
/// any non-ASCII byte simply fails `pred` and stops the run.
fn take_while(bytes: &[u8], start: usize, pred: impl Fn(u8) -> bool) -> usize {
    let mut end = start;
    while end < bytes.len() && pred(bytes[end]) {
        end += 1;
    }
    end
}

fn is_base64_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'='
}

fn is_base64url_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// Case-insensitive substring search — ASCII-only (the keyword we ever
/// search for, "secret", is ASCII), so a byte-lowercased comparison is
/// sufficient and avoids pulling in full Unicode case-folding for one
/// keyword check.
fn contains_ignore_case(text: &str, needle: &str) -> bool {
    let hay = text.to_ascii_lowercase();
    hay.contains(&needle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn categories(text: &str) -> Vec<&'static str> {
        find_all(text).into_iter().map(|(_, c)| c).collect()
    }

    #[test]
    fn aws_access_key_true_positive() {
        assert_eq!(
            categories("const KEY = \"AKIAIOSFODNN7EXAMPLE\""),
            vec!["aws_access_key"]
        );
    }

    #[test]
    fn aws_access_key_false_positive_wrong_length() {
        // 15 chars after AKIA, not 16 -- must not match.
        assert!(categories("AKIAIOSFODNN7EX").is_empty());
    }

    #[test]
    fn secret_key_near_keyword_true_positive() {
        let text = "secret_access_key = \"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\"";
        assert_eq!(categories(text), vec!["secret_key_near_keyword"]);
    }

    #[test]
    fn secret_key_false_positive_without_the_word_secret() {
        // Same 40-char base64 run, but no "secret" anywhere nearby.
        let text = "token = \"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\"";
        assert!(categories(text).is_empty());
    }

    #[test]
    fn private_key_pem_true_positive() {
        assert_eq!(
            categories("sig = \"-----BEGIN RSA PRIVATE KEY-----\""),
            vec!["private_key_pem"]
        );
    }

    #[test]
    fn private_key_pem_false_positive_public_key() {
        assert!(categories("\"-----BEGIN PUBLIC KEY-----\"").is_empty());
    }

    #[test]
    fn github_token_true_positive() {
        assert_eq!(
            categories("token = \"ghp_1234567890abcdef1234567890abcdef1234\""),
            vec!["github_token"]
        );
    }

    #[test]
    fn github_token_false_positive_too_short() {
        assert!(categories("\"ghp_short\"").is_empty());
    }

    #[test]
    fn gitlab_token_true_positive() {
        assert_eq!(
            categories("token = \"glpat-1234567890abcdefWXYZ\""),
            vec!["gitlab_token"]
        );
    }

    #[test]
    fn slack_token_true_positive() {
        assert_eq!(
            categories("token = \"xoxb-1234567890-abcdefghijklmnop\""),
            vec!["slack_token"]
        );
    }

    #[test]
    fn jwt_true_positive() {
        let text =
            "auth = \"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dQw4w9WgXcQ-rDpZ5RwFj0\"";
        assert_eq!(categories(text), vec!["jwt"]);
    }

    #[test]
    fn jwt_false_positive_missing_third_segment() {
        assert!(categories("\"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0\"").is_empty());
    }

    #[test]
    fn connection_string_true_positive() {
        assert_eq!(
            categories("dsn = \"postgres://admin:hunter2@db.internal:5432/orders\""),
            vec!["connection_string"]
        );
    }

    #[test]
    fn connection_string_false_positive_no_credentials() {
        assert!(categories("url = \"https://example.com/path\"").is_empty());
    }

    #[test]
    fn git_sha_is_not_flagged_by_any_pattern() {
        // A 40-hex-char git SHA is also 40 base64-alphabet chars, so it
        // could in principle trip find_secret_key_near_keyword -- but
        // only co-occurring with the word "secret", which a bare SHA
        // reference never does in these fixtures.
        assert!(categories("// see commit a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2").is_empty());
    }

    #[test]
    fn clean_signature_matches_nothing() {
        assert!(categories("fn parse_order(input: &str) -> Order").is_empty());
    }
}
