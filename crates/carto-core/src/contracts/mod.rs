//! Contract classification rules (ADR-0026, ADR-0027) — not spec
//! vocabulary. Decides what an extractor-recognized literal *means*
//! (`category`, producer/consumer `role`, `confidence`) from *where* it
//! sat (`RawLiteral::position`, `crate::lang::extractor`), so that
//! decision lives in one declarative place instead of being duplicated
//! per language. Built-in defaults cover the positions carto's own
//! extractors emit; a repo-local `.carto/contracts.json` can add more
//! categories over the same positions (open, repo-extensible category
//! vocabulary) but `role`/`lang` are validated against a closed set —
//! an unrecognized spelling is a hard error naming the bad value, not a
//! silently-ignored rule (ADR-0027's "closed vocabulary" — never widen
//! silently).

use crate::error::{Error, ErrorKind, Result};
use crate::graph::Confidence;
use crate::lang::Lang;
use serde::Deserialize;
use std::path::Path;

/// Repo-relative path of the optional classification-rule override file
/// (ADR-0027).
const CONFIG_RELPATH: &str = ".carto/contracts.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Producer,
    Consumer,
}

/// One classification rule: "a `lang`-extractor literal captured at
/// `position` is a `role` of `category`, with `confidence` (and
/// `evidence` prefix) reflecting how directly the syntax states that."
#[derive(Debug, Clone)]
struct ContractRule {
    category: String,
    role: Role,
    lang: Lang,
    position: String,
    confidence: Confidence,
    evidence: &'static str,
}

/// The full rule set: built-in defaults, plus whatever a repo's
/// `.carto/contracts.json` adds (ADR-0027). Repo rules are appended
/// *after* the built-ins in the list [`classify`](Self::classify) scans
/// front-to-back — so **built-in rules are matched first** (first match
/// wins). A repo can therefore only *add* a classification for a
/// position no built-in rule already claims; it can never override a
/// built-in classification's confidence/evidence for a position the
/// built-ins already cover.
#[derive(Debug)]
pub struct ContractRules {
    rules: Vec<ContractRule>,
}

impl ContractRules {
    /// Built-in rules only, no repo override — used by callers (tests,
    /// `carto contract`/`orphans` today) that don't need `.carto/
    /// contracts.json` at all.
    pub fn builtin() -> Self {
        ContractRules {
            rules: builtin_rules(),
        }
    }

    /// Built-in rules plus `<repo_root>/.carto/contracts.json` if
    /// present. `Err` on a present-but-malformed file (bad JSON, or a
    /// `role`/`lang` spelling outside the closed vocabulary) — a
    /// misconfigured classification file must stop the index, not
    /// silently classify nothing.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let mut rules = builtin_rules();
        let config_path = repo_root.join(CONFIG_RELPATH);
        let bytes = match std::fs::read(&config_path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ContractRules { rules });
            }
            Err(e) => {
                return Err(Error::with_source(
                    ErrorKind::UserError,
                    format!("failed to read `{}`", config_path.display()),
                    e,
                ));
            }
        };
        let doc: ConfigDoc = serde_json::from_slice(&bytes).map_err(|e| {
            Error::with_source(
                ErrorKind::UserError,
                format!("failed to parse `{}`", config_path.display()),
                e,
            )
        })?;
        for entry in doc.contracts {
            rules.push(entry.into_rule(&config_path)?);
        }
        Ok(ContractRules { rules })
    }

    /// First-match-wins lookup by `(lang, position)`. `None` means the
    /// extractor recognized this literal well enough to report a
    /// position for it, but no rule classifies that position — the
    /// literal is dropped (no `Contract` node, no edge), same "unmapped
    /// beats guessed" honesty as an unresolved call.
    pub fn classify(
        &self,
        lang: Lang,
        position: &str,
    ) -> Option<(&str, Role, Confidence, &'static str)> {
        self.rules
            .iter()
            .find(|r| r.lang == lang && r.position == position)
            .map(|r| (r.category.as_str(), r.role, r.confidence, r.evidence))
    }
}

#[derive(Deserialize)]
struct ConfigDoc {
    #[serde(default)]
    contracts: Vec<ConfigRule>,
}

#[derive(Deserialize)]
struct ConfigRule {
    category: String,
    role: String,
    lang: String,
    position: String,
}

impl ConfigRule {
    fn into_rule(self, config_path: &Path) -> Result<ContractRule> {
        let role = match self.role.as_str() {
            "producer" => Role::Producer,
            "consumer" => Role::Consumer,
            other => {
                return Err(Error::new(
                    ErrorKind::UserError,
                    format!(
                        "`{}`: unknown role `{other}` (expected `producer` or `consumer`)",
                        config_path.display()
                    ),
                ));
            }
        };
        let lang = parse_lang(&self.lang).ok_or_else(|| {
            Error::new(
                ErrorKind::UserError,
                format!("`{}`: unknown lang `{}`", config_path.display(), self.lang),
            )
        })?;
        Ok(ContractRule {
            category: self.category,
            role,
            lang,
            position: self.position,
            // A repo-declared rule is, by definition, an interpretation
            // the syntax itself doesn't state — never `certain`.
            confidence: Confidence::Inferred,
            evidence: "repo-config",
        })
    }
}

/// Parses a `.carto/contracts.json` `lang` string by reusing `Lang`'s
/// own `Deserialize` impl (`lang/mod.rs`'s `#[serde(rename_all =
/// "snake_case")]`, e.g. `"type_script"`/`"java_script"`, not
/// `"typescript"`/`"javascript"`) rather than a hand-duplicated match —
/// a second copy of that spelling table here would drift the moment a
/// `Lang` variant is added/renamed with no compiler link forcing this
/// function to follow. Also keeps this config vocabulary consistent
/// with what `graph.json`'s own `FileNode.lang` field actually spells,
/// instead of a bespoke second vocabulary a repo author would have to
/// learn separately.
fn parse_lang(s: &str) -> Option<Lang> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/// The two rules slice 1 needs (ADR-0026's acceptance test: CloudWatch
/// metric names). HCL's attribute capture states its claim outright
/// (`certain`); C#'s anonymous-object-member capture is this rule's own
/// interpretation of what an emission looks like, not something the
/// grammar states (`inferred`) — same reasoning ADR-0016 used for `new
/// Foo()` resolving to a type.
fn builtin_rules() -> Vec<ContractRule> {
    vec![
        ContractRule {
            category: "metric_name".to_string(),
            role: Role::Producer,
            lang: Lang::CSharp,
            position: "object-init:Name".to_string(),
            confidence: Confidence::Inferred,
            evidence: "csharp-object-init",
        },
        ContractRule {
            category: "metric_name".to_string(),
            role: Role::Consumer,
            lang: Lang::Hcl,
            position: "aws_cloudwatch_metric_alarm.metric_name".to_string(),
            confidence: Confidence::Certain,
            evidence: "hcl-attr",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_classify_the_slice_1_positions() {
        let rules = ContractRules::builtin();
        let (category, role, confidence, _) = rules
            .classify(Lang::CSharp, "object-init:Name")
            .expect("csharp object-init:Name must classify");
        assert_eq!(category, "metric_name");
        assert!(matches!(role, Role::Producer));
        assert!(matches!(confidence, Confidence::Inferred));

        let (category, role, confidence, _) = rules
            .classify(Lang::Hcl, "aws_cloudwatch_metric_alarm.metric_name")
            .expect("hcl metric_name attribute must classify");
        assert_eq!(category, "metric_name");
        assert!(matches!(role, Role::Consumer));
        assert!(matches!(confidence, Confidence::Certain));
    }

    #[test]
    fn unclassified_position_is_none() {
        let rules = ContractRules::builtin();
        assert!(rules.classify(Lang::CSharp, "object-init:Unit").is_none());
        assert!(rules.classify(Lang::Rust, "object-init:Name").is_none());
    }

    #[test]
    fn missing_config_file_falls_back_to_builtins_only() {
        let dir = std::env::temp_dir().join(format!(
            "carto-contracts-test-missing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let rules = ContractRules::load(&dir).unwrap();
        assert!(
            rules
                .classify(Lang::Hcl, "aws_cloudwatch_metric_alarm.metric_name")
                .is_some()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repo_config_extends_the_rule_set() {
        let dir = std::env::temp_dir().join(format!(
            "carto-contracts-test-extend-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join(".carto")).unwrap();
        std::fs::write(
            dir.join(".carto/contracts.json"),
            r#"{"contracts": [
                {"category": "env_var", "role": "consumer", "lang": "csharp", "position": "call-arg:GetEnvironmentVariable"}
            ]}"#,
        )
        .unwrap();
        let rules = ContractRules::load(&dir).unwrap();
        let (category, role, ..) = rules
            .classify(Lang::CSharp, "call-arg:GetEnvironmentVariable")
            .expect("repo-added rule must classify");
        assert_eq!(category, "env_var");
        assert!(matches!(role, Role::Consumer));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Regression guard: `parse_lang` must accept every spelling
    /// `Lang`'s own `Deserialize` accepts, including ones a
    /// hand-duplicated match table previously omitted (`yaml`) or got
    /// wrong (`Lang::TypeScript`/`Lang::JavaScript` serialize as
    /// `type_script`/`java_script` under `#[serde(rename_all =
    /// "snake_case")]`, not `typescript`/`javascript`).
    #[test]
    fn lang_parses_every_spelling_langs_own_serde_accepts() {
        let dir = std::env::temp_dir().join(format!(
            "carto-contracts-test-langs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join(".carto")).unwrap();
        std::fs::write(
            dir.join(".carto/contracts.json"),
            r#"{"contracts": [
                {"category": "x", "role": "consumer", "lang": "yaml", "position": "p1"},
                {"category": "y", "role": "consumer", "lang": "type_script", "position": "p2"}
            ]}"#,
        )
        .unwrap();
        let rules = ContractRules::load(&dir).unwrap();
        assert!(rules.classify(Lang::Yaml, "p1").is_some());
        assert!(rules.classify(Lang::TypeScript, "p2").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_role_is_a_user_error() {
        let dir = std::env::temp_dir().join(format!(
            "carto-contracts-test-badrole-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join(".carto")).unwrap();
        std::fs::write(
            dir.join(".carto/contracts.json"),
            r#"{"contracts": [{"category": "x", "role": "bogus", "lang": "csharp", "position": "p"}]}"#,
        )
        .unwrap();
        let err = ContractRules::load(&dir).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UserError);
        std::fs::remove_dir_all(&dir).ok();
    }
}
