//! Question protocol: the shared shape guided flows use to ask for input,
//! either interactively (Task 2's `interactive` wrappers) or scripted via
//! `--answer` / `--answers-file` (this task).
//!
//! A command that needs input builds one or more [`Question`]s and asks an
//! [`Asker`] for answers. In a script/agent context, [`ScriptedAsker`]
//! answers from a pre-supplied map and, when something is missing, returns
//! [`NeedsInput`] — a structured error the CLI turns into a `needs-input`
//! JSON document on stdout and exit code 6, so a caller can answer the
//! missing questions and re-run.
//!
//! This module lays the primitives down; the interactive `Asker` and the
//! guided flows that build `Question`s live in follow-up tasks, so several
//! items here have no caller yet outside tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// Id prefixes every `--answer <id>=<value>` is validated against at
/// startup. Later tasks and workstreams append their own prefixes
/// (`binding.`, `env.`, `promote.`, `auth.`, `new.`, …).
pub const KNOWN_ID_PREFIXES: &[&str] = &["confirm.protected."];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionKind {
    Choice,
    Text,
    Confirm,
    ConfirmEnv,
}

impl QuestionKind {
    fn as_str(self) -> &'static str {
        match self {
            QuestionKind::Choice => "choice",
            QuestionKind::Text => "text",
            QuestionKind::Confirm => "confirm",
            QuestionKind::ConfirmEnv => "confirm-env",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub kind: QuestionKind,
    pub prompt: String,
    pub candidates: Vec<Candidate>,
    pub allow_other: bool,
    pub default: Option<String>,
}

impl Question {
    pub fn choice(
        id: impl Into<String>,
        prompt: impl Into<String>,
        candidates: Vec<Candidate>,
    ) -> Self {
        Question {
            id: id.into(),
            kind: QuestionKind::Choice,
            prompt: prompt.into(),
            candidates,
            allow_other: false,
            default: None,
        }
    }

    pub fn text(id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Question {
            id: id.into(),
            kind: QuestionKind::Text,
            prompt: prompt.into(),
            candidates: Vec::new(),
            allow_other: false,
            default: None,
        }
    }

    pub fn confirm(id: impl Into<String>, prompt: impl Into<String>, default_yes: bool) -> Self {
        Question {
            id: id.into(),
            kind: QuestionKind::Confirm,
            prompt: prompt.into(),
            candidates: Vec::new(),
            allow_other: false,
            default: Some(if default_yes { "yes" } else { "no" }.to_string()),
        }
    }

    /// A protected-environment confirmation. `id` = `confirm.protected.<env>`;
    /// the answer must equal `env` exactly (see [`coerce`]).
    pub fn confirm_env(env: &str, operation: &str) -> Self {
        Question {
            id: format!("confirm.protected.{env}"),
            kind: QuestionKind::ConfirmEnv,
            prompt: format!(
                "Environment '{env}' is protected. Type its name to confirm {operation}:"
            ),
            candidates: Vec::new(),
            allow_other: false,
            default: None,
        }
    }

    pub fn with_default(mut self, d: impl Into<String>) -> Self {
        self.default = Some(d.into());
        self
    }

    pub fn allow_other(mut self) -> Self {
        self.allow_other = true;
        self
    }

    /// Hand-built serialization for the `needs-input` protocol document:
    /// `candidates` is omitted when empty, `default` when `None`, and
    /// `allow_other` is present only when `true`.
    fn to_value(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), json!(self.id));
        obj.insert("kind".to_string(), json!(self.kind.as_str()));
        obj.insert("prompt".to_string(), json!(self.prompt));
        if !self.candidates.is_empty() {
            obj.insert(
                "candidates".to_string(),
                json!(
                    self.candidates
                        .iter()
                        .map(|c| json!({"value": c.value, "label": c.label}))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if self.allow_other {
            obj.insert("allow_other".to_string(), json!(true));
        }
        if let Some(default) = &self.default {
            obj.insert("default".to_string(), json!(default));
        }
        Value::Object(obj)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Choice(String),
    Text(String),
    Confirm(bool),
}

impl Answer {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Answer::Choice(s) | Answer::Text(s) => Some(s.as_str()),
            Answer::Confirm(_) => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Answer::Confirm(b) => Some(*b),
            _ => None,
        }
    }
}

pub trait Asker {
    fn ask(&mut self, q: &Question) -> Result<Answer>;
    fn ask_all(&mut self, qs: &[Question]) -> Result<Vec<Answer>>;
}

/// Answers a fixed set of [`Question`]s from a pre-supplied map
/// (`--answer id=value` / `--answers-file`). Anything missing is collected
/// into a single [`NeedsInput`] error rather than failing on the first gap,
/// so a caller sees every outstanding question in one round-trip.
pub struct ScriptedAsker {
    answers: BTreeMap<String, String>,
    command: String,
    context: Value,
}

impl ScriptedAsker {
    pub fn new(
        answers: BTreeMap<String, String>,
        command: impl Into<String>,
        context: Value,
    ) -> Self {
        ScriptedAsker {
            answers,
            command: command.into(),
            context,
        }
    }

    fn needs_input(&self, questions: Vec<Question>) -> anyhow::Error {
        anyhow::Error::new(NeedsInput {
            command: self.command.clone(),
            context: self.context.clone(),
            questions,
        })
    }
}

impl Asker for ScriptedAsker {
    fn ask(&mut self, q: &Question) -> Result<Answer> {
        match self.answers.get(&q.id) {
            Some(raw) => coerce(q, raw),
            None => Err(self.needs_input(vec![q.clone()])),
        }
    }

    fn ask_all(&mut self, qs: &[Question]) -> Result<Vec<Answer>> {
        let mut answers = Vec::with_capacity(qs.len());
        let mut missing = Vec::new();
        for q in qs {
            match self.answers.get(&q.id) {
                Some(raw) => answers.push(coerce(q, raw)?),
                None => missing.push(q.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(self.needs_input(missing));
        }
        Ok(answers)
    }
}

/// Structured "I need more input" error. The CLI prints [`Self::to_json`]
/// to stdout and maps this to exit code 6.
#[derive(Debug, thiserror::Error)]
pub struct NeedsInput {
    pub command: String,
    pub context: Value,
    pub questions: Vec<Question>,
}

impl std::fmt::Display for NeedsInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids = self
            .questions
            .iter()
            .map(|q| q.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "{} question(s) need an answer: {ids}",
            self.questions.len()
        )
    }
}

impl NeedsInput {
    /// The `needs-input` protocol document printed to stdout.
    pub fn to_json(&self) -> Value {
        json!({
            "status": "needs-input",
            "command": self.command,
            "context": self.context,
            "questions": self.questions.iter().map(Question::to_value).collect::<Vec<_>>(),
        })
    }
}

/// Parse a `--answer id=value` flag.
pub fn parse_answer_flag(s: &str) -> Result<(String, String)> {
    match s.split_once('=') {
        Some((id, value)) if !id.is_empty() => Ok((id.to_string(), value.to_string())),
        _ => bail!("invalid --answer '{s}': expected <id>=<value>"),
    }
}

/// Load answers from an optional JSON file (`{"<id>": "<value>", ...}`)
/// merged with `--answer` flags, which take precedence over the file.
pub fn load_answers(flags: &[String], file: Option<&Path>) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    if let Some(path) = file {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading answers file '{}': {e}", path.display()))?;
        let parsed: BTreeMap<String, String> = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing answers file '{}': {e}", path.display()))?;
        map.extend(parsed);
    }
    for flag in flags {
        let (id, value) = parse_answer_flag(flag)?;
        map.insert(id, value);
    }
    Ok(map)
}

/// Coerce a raw string answer into the [`Answer`] variant appropriate for
/// `q.kind`, validating against candidates / confirm semantics / the
/// protected-env name as needed.
pub fn coerce(q: &Question, raw: &str) -> Result<Answer> {
    match q.kind {
        QuestionKind::Choice => {
            if q.candidates.iter().any(|c| c.value == raw) || q.allow_other {
                Ok(Answer::Choice(raw.to_string()))
            } else {
                bail!(
                    "invalid answer for '{}': '{raw}' is not one of the offered candidates",
                    q.id
                )
            }
        }
        QuestionKind::Text => Ok(Answer::Text(raw.to_string())),
        QuestionKind::Confirm => match raw.to_ascii_lowercase().as_str() {
            "yes" | "y" | "true" => Ok(Answer::Confirm(true)),
            "no" | "n" | "false" => Ok(Answer::Confirm(false)),
            _ => bail!("invalid answer for '{}': expected yes/no", q.id),
        },
        QuestionKind::ConfirmEnv => {
            let env =
                q.id.strip_prefix("confirm.protected.")
                    .unwrap_or(q.id.as_str());
            if raw == env {
                Ok(Answer::Confirm(true))
            } else {
                bail!(
                    "invalid answer for '{}': must type the environment name '{env}' exactly",
                    q.id
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_asker_answers_from_map_and_collects_the_rest() {
        let mut asker = ScriptedAsker::new(
            [("binding.prod.docs-storage".to_string(), "same".to_string())]
                .into_iter()
                .collect(),
            "promote",
            json!({"from": "dev", "to": "prod"}),
        );
        let q1 = Question::choice(
            "binding.prod.docs-storage",
            "Which storage?",
            vec![Candidate {
                value: "same".into(),
                label: "same as dev".into(),
            }],
        );
        let q2 = Question::text("binding.prod.enrich-fn", "Function app for prod?");
        assert_eq!(asker.ask(&q1).unwrap().as_str(), Some("same"));
        let err = asker.ask_all(&[q1.clone(), q2.clone()]).unwrap_err();
        let ni = err.downcast_ref::<NeedsInput>().expect("NeedsInput");
        assert_eq!(
            ni.questions
                .iter()
                .map(|q| q.id.as_str())
                .collect::<Vec<_>>(),
            vec!["binding.prod.enrich-fn"]
        );
        let doc = ni.to_json();
        assert_eq!(doc["status"], "needs-input");
        assert_eq!(doc["command"], "promote");
        assert_eq!(doc["questions"][0]["kind"], "text");
        assert!(
            doc["questions"][0].get("candidates").is_none(),
            "omitted when empty"
        );
    }

    #[test]
    fn coerce_enforces_candidates_and_confirm_env() {
        let q = Question::choice(
            "q",
            "?",
            vec![Candidate {
                value: "a".into(),
                label: "A".into(),
            }],
        );
        assert!(coerce(&q, "b").is_err());
        assert_eq!(
            coerce(&q.clone().allow_other(), "b").unwrap().as_str(),
            Some("b")
        );
        let c = Question::confirm("c", "?", true);
        assert_eq!(coerce(&c, "no").unwrap().as_bool(), Some(false));
        let e = Question::confirm_env("prod", "push");
        assert_eq!(e.id, "confirm.protected.prod");
        assert!(coerce(&e, "prd").is_err());
        assert_eq!(coerce(&e, "prod").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn load_answers_merges_flags_over_file_and_rejects_bad_flags() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.json");
        std::fs::write(&f, r#"{"x": "1", "y": "2"}"#).unwrap();
        let m = load_answers(&["y=3".to_string()], Some(&f)).unwrap();
        assert_eq!(m["x"], "1");
        assert_eq!(m["y"], "3");
        assert!(parse_answer_flag("novalue").is_err());
    }
}
