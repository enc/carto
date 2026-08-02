# secrets-corpus fixture

Synthetic corpus for the redaction pass's acceptance tests (spec §7.5,
§11.1, ADR-0017). Not real customer code — every "secret" below is
fake/placeholder-shaped, per spec §11.1's own example (`AKIA` +
`EXAMPLE`). Two files across two already-supported languages
(Rust, Go) to prove redaction is language-agnostic: it scans
`SymbolNode.signature` post-extraction, uniformly, regardless of which
extractor produced it.

| Symbol (file) | Category | Note |
|---|---|---|
| `AWS_ACCESS_KEY` (config.rs) | `aws_access_key` | `AKIA` + 16 uppercase-alnum chars |
| `AWS_SECRET_ACCESS_KEY` (config.rs) | `secret_key_near_keyword` | 40-char base64 run, co-occurring with "secret" in the same declaration |
| `GITHUB_TOKEN` (config.rs) | `github_token` | `ghp_` prefix |
| `TLS_KEY_HEADER` (config.rs) | `private_key_pem` | PEM header literal |
| `SESSION_TOKEN` (config.rs) | `high_entropy` | 32-char random-looking token matching no fixed pattern — caught by the Shannon-entropy pass instead |
| `GIT_COMMIT_REF` (config.rs) | *(clean — must not be redacted)* | 40-hex-char, git-SHA-shaped; hex's max entropy (4.0 bits/char) is below the 4.2 threshold, and no pattern matches it |
| `SERVER_PORT` (config.rs) | *(clean)* | ordinary low-entropy constant |
| `dbConnection` (service.go) | `connection_string` | `scheme://user:pass@host` — only the credential-bearing prefix is redacted, the `/orders` path segment is left alone |
| `slackWebhookToken` (service.go) | `slack_token` | `xoxb-` prefix |
| `gitlabToken` (service.go) | `gitlab_token` | `glpat-` prefix |
| `jwtExample` (service.go) | `jwt` | `eyJ...` three-segment base64url |
| `serviceName` (service.go) | *(clean)* | ordinary low-entropy constant |

Every category in spec §7.5(a)/(b) fires exactly once (`manifest.json`'s
`redaction.by_category` has all 9 keys, each `1`); every "clean" symbol
above is byte-identical to its source declaration in `graph.json`.

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/secrets-corpus --out /tmp/carto-secrets --json
python3 -m json.tool /tmp/carto-secrets/graph.json
python3 -m json.tool /tmp/carto-secrets/manifest.json
```
