//! `rigg dev docs-check` — the mechanical honesty check for the docs.
//!
//! It has to live in the binary: the `rigg` crate has no lib target, so
//! `Cli::try_parse_from` (check 1) and `ask::KNOWN_ID_PREFIXES` (check 4)
//! are only reachable from here. `crates/rigg/tests/docs_guards.rs` drives
//! it with `--root <workspace>` and asserts the `docs-check: ok` verdict.
//!
//! Four checks, all reported together so one run lists everything:
//!
//! 1. every `rigg …` line in a shell code fence parses through clap;
//! 2. every relative Markdown link — and its `#anchor` — resolves;
//! 3. every `RIGG_*` / `AZURE_*` variable the code reads is documented;
//! 4. every `ask::KNOWN_ID_PREFIXES` entry is documented.
//!
//! Checks 3 and 4 are skipped with a note until their reference pages
//! exist; they become errors once the pages land.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;
use crate::commands::ask::KNOWN_ID_PREFIXES;

const ENV_VARS_PAGE: &str = "docs/reference/environment-variables.md";
const QUESTIONS_PAGE: &str = "docs/reference/exit-codes-and-questions.md";

/// Design documents, not user documentation: they describe commands that
/// are proposed rather than built, so parsing them would be meaningless.
const SKIPPED_DIRS: &[&str] = &["superpowers"];

pub fn run(root: Option<PathBuf>) -> Result<()> {
    let root = match root {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let root = root
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("--root {}: {e}", root.display()))?;

    let files = doc_files(&root);
    if files.is_empty() {
        anyhow::bail!(
            "docs-check: no documentation files found under {}",
            root.display()
        );
    }

    let mut findings: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut commands = 0usize;
    let mut links = 0usize;

    for file in &files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        let shown = display_path(&root, file);
        commands += check_commands(&shown, &text, &mut findings);
        links += check_links(&root, file, &shown, &text, &mut findings);
    }

    check_env_vars(&root, &mut findings, &mut notes)?;
    check_question_prefixes(&root, &mut findings, &mut notes)?;

    println!(
        "docs-check: {} files, {commands} rigg command lines, {links} relative links",
        files.len()
    );
    for note in &notes {
        println!("note: {note}");
    }
    if findings.is_empty() {
        println!("docs-check: ok");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    anyhow::bail!("docs-check: {} problem(s)", findings.len())
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

fn doc_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for name in ["README.md", "GETTING_STARTED.md", "CONCEPTS.md", "MCP.md"] {
        let path = root.join(name);
        if path.is_file() {
            files.push(path);
        }
    }
    collect_markdown(&root.join("docs"), &mut files);
    collect_markdown(&root.join("samples"), &mut files);

    let skills = root.join(".claude").join("skills");
    if let Ok(entries) = std::fs::read_dir(&skills) {
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let skill = dir.join("SKILL.md");
            if skill.is_file() {
                files.push(skill);
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !SKIPPED_DIRS.contains(&name) {
                collect_markdown(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn display_path(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

// ---------------------------------------------------------------------------
// 1. Command parse
// ---------------------------------------------------------------------------

/// Parse every `rigg …` line in a shell fence; returns how many were checked.
fn check_commands(shown: &str, text: &str, findings: &mut Vec<String>) -> usize {
    let mut checked = 0;
    let mut in_shell_fence = false;
    let mut in_other_fence = false;
    // A `\`-continued command and the line it started on.
    let mut pending: Option<(usize, String)> = None;

    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();

        if trimmed.starts_with("```") {
            if in_shell_fence || in_other_fence {
                in_shell_fence = false;
                in_other_fence = false;
            } else {
                let tag = trimmed
                    .trim_start_matches('`')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if matches!(tag, "" | "bash" | "sh" | "shell") {
                    in_shell_fence = true;
                } else {
                    in_other_fence = true;
                }
            }
            pending = None;
            continue;
        }
        if !in_shell_fence {
            continue;
        }

        let mut line = trimmed.to_string();
        if let Some(rest) = line.strip_prefix("$ ") {
            line = rest.trim().to_string();
        }

        let (start_line, mut command) = match pending.take() {
            Some((start, acc)) => (start, format!("{acc} {line}")),
            None => {
                if first_token(&line) != Some("rigg") {
                    continue;
                }
                (line_no, line)
            }
        };

        if let Some(head) = command.strip_suffix('\\') {
            pending = Some((start_line, head.trim_end().to_string()));
            continue;
        }

        command = strip_comment(&command);
        if command.trim().is_empty() {
            continue;
        }
        // Elisions are prose, not a command a reader can paste.
        if command.contains('…') || command.contains("...") {
            continue;
        }

        checked += 1;
        if let Err(err) = parse_command(&command) {
            findings.push(format!("{shown}:{start_line}: {err}"));
        }
    }
    checked
}

fn first_token(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// Drop a trailing `# comment`, ignoring `#` inside quotes.
fn strip_comment(line: &str) -> String {
    let mut quote: Option<char> = None;
    let mut prev_is_space = true;
    for (i, ch) in line.char_indices() {
        match ch {
            '\'' | '"' => match quote {
                Some(q) if q == ch => quote = None,
                Some(_) => {}
                None => quote = Some(ch),
            },
            '#' if quote.is_none() && prev_is_space => return line[..i].trim_end().to_string(),
            _ => {}
        }
        prev_is_space = ch.is_whitespace();
    }
    line.trim_end().to_string()
}

/// `<placeholder>` stands for a value the reader supplies; any value parses.
fn substitute_placeholders(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(rel) => {
                out.push_str(&rest[..open]);
                out.push('x');
                rest = &rest[open + rel + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Shell plumbing the parser must not see: everything from the first
/// operator token onwards belongs to the shell, not to rigg.
fn is_shell_operator(token: &str) -> bool {
    matches!(
        token,
        "|" | "||" | "&&" | ";" | ">" | ">>" | "<" | "2>" | "2>&1" | "&"
    )
}

fn parse_command(command: &str) -> Result<(), String> {
    let line = substitute_placeholders(command);
    let tokens = shlex::split(&line).ok_or_else(|| format!("unbalanced quotes in `{command}`"))?;
    let tokens: Vec<String> = tokens
        .into_iter()
        .take_while(|t| !is_shell_operator(t))
        .collect();
    if tokens.is_empty() {
        return Ok(());
    }
    match Cli::try_parse_from(&tokens) {
        Ok(_) => Ok(()),
        Err(err) => match err.kind() {
            // `--help` / `--version` are legitimate things to show.
            clap::error::ErrorKind::DisplayHelp
            | clap::error::ErrorKind::DisplayVersion
            | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => Ok(()),
            _ => Err(one_line(&err.render().to_string())),
        },
    }
}

fn one_line(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("could not be parsed")
        .to_string()
}

// ---------------------------------------------------------------------------
// 2. Relative links
// ---------------------------------------------------------------------------

/// Check every relative link in `text`; returns how many were checked.
fn check_links(
    root: &Path,
    file: &Path,
    shown: &str,
    text: &str,
    findings: &mut Vec<String>,
) -> usize {
    let dir = file.parent().unwrap_or(root);
    let mut checked = 0;

    for (idx, line) in text.lines().enumerate() {
        for target in link_targets(line) {
            let target = target.trim();
            if target.is_empty()
                || target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with("tel:")
                || target.starts_with("//")
            {
                continue;
            }
            checked += 1;
            let (path_part, anchor) = match target.split_once('#') {
                Some((p, a)) => (p, Some(a)),
                None => (target, None),
            };
            let resolved = if path_part.is_empty() {
                file.to_path_buf()
            } else {
                dir.join(path_part)
            };
            if !resolved.exists() {
                findings.push(format!(
                    "{shown}:{}: link target `{target}` does not exist",
                    idx + 1
                ));
                continue;
            }
            let Some(anchor) = anchor else { continue };
            if anchor.is_empty() || resolved.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(target_text) = std::fs::read_to_string(&resolved) else {
                continue;
            };
            if !heading_slugs(&target_text).contains(anchor) {
                findings.push(format!(
                    "{shown}:{}: `{target}` — no heading with anchor `#{anchor}`",
                    idx + 1
                ));
            }
        }
    }
    checked
}

/// The `(target)` of every `[text](target)` / `![alt](target)` on a line.
fn link_targets(line: &str) -> Vec<String> {
    let bytes: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == ']' && bytes[i + 1] == '(' {
            let mut depth = 1;
            let mut j = i + 2;
            let mut target = String::new();
            while j < bytes.len() {
                match bytes[j] {
                    '(' => {
                        depth += 1;
                        target.push('(');
                    }
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        target.push(')');
                    }
                    c => target.push(c),
                }
                j += 1;
            }
            if depth == 0 {
                // A link may carry a title: `(path "Title")`.
                let target = target
                    .split_once(" \"")
                    .map(|(p, _)| p.to_string())
                    .unwrap_or(target);
                out.push(target.trim_matches(|c| c == '<' || c == '>').to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Every heading anchor a Markdown file offers, with GitHub's duplicate
/// suffixes (`title`, `title-1`, `title-2`, …). Fenced blocks are skipped so
/// a `# comment` in a shell example is not mistaken for a heading.
fn heading_slugs(text: &str) -> BTreeSet<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = BTreeSet::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let title = trimmed.trim_start_matches('#');
        if !title.starts_with(' ') && !title.is_empty() {
            continue; // `#tag`, not a heading
        }
        let base = crate::commands::docgen::slug(title.trim());
        let count = seen.iter().filter(|s| **s == base).count();
        seen.push(base.clone());
        out.insert(if count == 0 {
            base
        } else {
            format!("{base}-{count}")
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 3. Environment variables
// ---------------------------------------------------------------------------

fn check_env_vars(root: &Path, findings: &mut Vec<String>, notes: &mut Vec<String>) -> Result<()> {
    let page = root.join(ENV_VARS_PAGE);
    let names = env_var_names(&root.join("crates"));
    if !page.is_file() {
        notes.push(format!(
            "{ENV_VARS_PAGE} does not exist yet — {} environment variables unchecked",
            names.len()
        ));
        return Ok(());
    }
    let text = std::fs::read_to_string(&page)?;
    for name in names {
        if !text.contains(&name) {
            findings.push(format!(
                "{ENV_VARS_PAGE}: `{name}` is read by the code but not documented"
            ));
        }
    }
    Ok(())
}

/// Every `RIGG_*` / `AZURE_*` identifier appearing in the crates' sources.
fn env_var_names(crates: &Path) -> BTreeSet<String> {
    let mut sources = Vec::new();
    collect_rust(crates, &mut sources);
    let mut out = BTreeSet::new();
    for path in sources {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for prefix in ["RIGG_", "AZURE_"] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at..];
                let end = tail
                    .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                    .unwrap_or(tail.len());
                let name = &tail[..end];
                // `RIGG_` alone, or a trailing `_`, is a fragment.
                if name.len() > prefix.len() && !name.ends_with('_') {
                    out.insert(name.to_string());
                }
                rest = &tail[end.max(1)..];
            }
        }
    }
    out
}

fn collect_rust(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name != "target" && name != "tests" && name != "fixtures" {
                collect_rust(&path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Question id prefixes
// ---------------------------------------------------------------------------

fn check_question_prefixes(
    root: &Path,
    findings: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let page = root.join(QUESTIONS_PAGE);
    if !page.is_file() {
        notes.push(format!(
            "{QUESTIONS_PAGE} does not exist yet — {} question id prefixes unchecked",
            KNOWN_ID_PREFIXES.len()
        ));
        return Ok(());
    }
    let text = std::fs::read_to_string(&page)?;
    for prefix in KNOWN_ID_PREFIXES {
        if !text.contains(prefix) {
            findings.push(format!(
                "{QUESTIONS_PAGE}: question id prefix `{prefix}` is not documented"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_shell_fences_are_scanned() {
        let text = "```json\nrigg not-a-command\n```\n\n```bash\nrigg status\n```\n";
        let mut findings = Vec::new();
        assert_eq!(check_commands("x.md", text, &mut findings), 1);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_bad_flag_is_reported_with_file_and_line() {
        let text = "```bash\nrigg status --nope\n```\n";
        let mut findings = Vec::new();
        check_commands("x.md", text, &mut findings);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].starts_with("x.md:2: "), "{}", findings[0]);
    }

    #[test]
    fn placeholders_continuations_comments_and_redirects_are_handled() {
        let text = "```bash\n\
                    $ rigg push <project> --prune   # comment\n\
                    rigg promote <project> \\\n  --from <dev> --to <prod>\n\
                    rigg completion zsh > ~/.zfunc/_rigg\n\
                    rigg status …\n\
                    ```\n";
        let mut findings = Vec::new();
        // The elided line is skipped, the other three are checked.
        assert_eq!(check_commands("x.md", text, &mut findings), 3);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn non_rigg_lines_are_ignored() {
        let text = "```bash\naz login\nCOMPLETE=zsh rigg\n```\n";
        let mut findings = Vec::new();
        assert_eq!(check_commands("x.md", text, &mut findings), 0);
        assert!(findings.is_empty());
    }

    #[test]
    fn heading_slugs_skip_fences_and_number_duplicates() {
        let text = "# Title\n```bash\n# not a heading\n```\n## Env\n## Env\n";
        let slugs = heading_slugs(text);
        assert!(slugs.contains("title"));
        assert!(slugs.contains("env"));
        assert!(slugs.contains("env-1"));
        assert!(!slugs.contains("not-a-heading"));
    }

    #[test]
    fn link_targets_are_extracted_without_titles_or_images() {
        let line = r#"See [a](docs/a.md#x) and ![img](media/logo.png "Logo")."#;
        assert_eq!(link_targets(line), vec!["docs/a.md#x", "media/logo.png"]);
    }

    #[test]
    fn env_var_scan_finds_real_names_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "std::env::var(\"RIGG_ENV\"); \"AZURE_TENANT_ID\"; RIGG_",
        )
        .unwrap();
        let names = env_var_names(dir.path());
        assert!(names.contains("RIGG_ENV"));
        assert!(names.contains("AZURE_TENANT_ID"));
        assert_eq!(names.len(), 2);
    }
}
