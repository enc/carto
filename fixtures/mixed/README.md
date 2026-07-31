# mixed fixture

Synthetic repo for M1.b.1's `walk` acceptance tests (spec §11.1). Not real
customer code — every file here is deliberately fake and minimal, sized
only to exercise one specific walk classification:

| Path | Exercises |
|---|---|
| `src/main.rs`, `src/app.ts`, `lib.py` | ordinary indexed files (multiple v1 languages) |
| `node_modules/`, `target/`, `__pycache__/` | built-in directory denylist (spec §5.1), pruned even under `--no-gitignore` |
| `dist/` | `.gitignore`-only exclusion (present here, absent from the built-in denylist) |
| `generated/` | `.cartoignore`-only exclusion, additive and independent of `.gitignore` |
| `secrets.local` | `.gitignore` pattern match (`*.local`), distinct from the sensitive set |
| `binary.dat` | binary sniff (NUL in first 8 KiB) |
| `vendor.min.js` | `skipped: "minified"` |
| `vendor.lock` | `skipped: "lock_file"` |

Deliberately **not** included here: `.env`/`*.pem`/`*credentials*`-shaped
files. This repo's own tooling refuses to write files matching those
names, fake content or not — a reasonable blanket guard, so it isn't
worth routing around for a fixture. That classification path
(`excluded: "sensitive"`, contents never read) is covered instead by
`crates/carto-core/src/walk/mod.rs`'s own unit tests, which build
temporary directories rather than committed fixture files.
