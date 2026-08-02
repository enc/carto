//! Synthetic constants for the redaction fixture (spec §7.5, §11.1,
//! ADR-0017). Every "secret" below is fake/placeholder-shaped, per
//! spec §11.1's own example (`AKIA` + `EXAMPLE`) — not real customer
//! code, not a real credential.

pub const AWS_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
pub const AWS_SECRET_ACCESS_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
pub const GITHUB_TOKEN: &str = "ghp_1234567890abcdef1234567890abcdef1234";
pub const TLS_KEY_HEADER: &str = "-----BEGIN RSA PRIVATE KEY-----";
pub const SESSION_TOKEN: &str = "xJ4kLpQ9rT2vN8mF6wZ1yB3cH5dS7aE";

/// Clean: a git-SHA-shaped 40-hex-char constant. Must NOT be redacted —
/// hex's max entropy (4.0 bits/char over a 16-symbol alphabet) is below
/// the 4.2 threshold regardless of "randomness", and it matches no
/// pattern (no AWS/token prefix, no "secret" anywhere in this line).
pub const GIT_COMMIT_REF: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

/// Clean: an ordinary low-entropy constant.
pub const SERVER_PORT: u16 = 8080;
