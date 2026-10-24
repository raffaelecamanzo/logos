//! The per-project coverage **execution consent** ([FR-CV-13], [ADR-87] §2,
//! S-736).
//!
//! A coverage run is not like a scan: it builds the project and runs its
//! tests, so it executes the project's own code, and it may reach the network
//! to fetch dependencies. No run starts in a project until the user has
//! consented to it there, and the consent names what it covers:
//!
//! - the **tools** that will run, by registry name — a tool not named yet is
//!   asked for, alone;
//! - the **custom command** (`[coverage_ingest].refresh_cmd`), by the SHA-256
//!   of its exact text, recorded as [`CUSTOM_TOOL`]. The command lives in the
//!   committed config, so a clone can change it: any change to the text asks
//!   again, showing the new text verbatim — its terminal control characters
//!   escaped ([`shown`]), so what is shown is what runs.
//!
//! The record lives in the user-level store
//! ([`load_coverage_consent`](crate::config::load_coverage_consent)), keyed by
//! the canonical project root and never inside the repository, apart from the
//! security scan consent. A run without `consent` whose consent is missing
//! refuses with exit 2 ([`ConfigError`]), naming `--consent`, and starts
//! nothing; with `consent` it records what was missing, then runs. An MCP
//! client cannot pass `consent`: it relays the instruction.
//!
//! [FR-CV-13]: ../../../docs/specs/requirements/FR-CV-13.md
//! [ADR-87]: ../../../docs/specs/architecture/decisions/ADR-87.md

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::config::{load_coverage_consent, record_coverage_consent, ConfigError, CoverageConsent};

/// The name the custom entry — the configured `refresh_cmd` — is consented
/// to under.
pub const CUSTOM_TOOL: &str = "custom";

/// The SHA-256 (lower-case hex) of `command`'s exact text: what a consent
/// pins.
pub fn refresh_cmd_sha256(command: &str) -> String {
    format!("{:x}", Sha256::digest(command.as_bytes()))
}

/// What a run would have to ask consent for before it starts anything: also
/// what the web UI's consent dialog shows (S-738).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct PendingConsent {
    /// The tools the project's consent does not cover yet, in the order the
    /// run names them; [`CUSTOM_TOOL`] when the custom command is not covered.
    pub tools: Vec<String>,
    /// The custom command's exact text, when it is not covered.
    pub refresh_cmd: Option<String>,
    /// Whether a custom command was consented to before and its text has
    /// changed since.
    pub command_changed: bool,
}

/// A consent dialog's answer (S-738): the tools the user accepted —
/// [`CUSTOM_TOOL`] for the custom command — and the custom command's text as
/// the dialog showed it.
#[derive(Debug, Clone, Copy)]
pub struct ConsentAnswer<'a> {
    pub tools: &'a [String],
    pub refresh_cmd: Option<&'a str>,
}

impl PendingConsent {
    /// Whether the recorded consent covers the whole run.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether `answer` covers all of this: every pending tool among its
    /// tools, and — when the custom command is pending — its exact text.
    pub fn accepted_by(&self, answer: &ConsentAnswer<'_>) -> bool {
        self.tools.iter().all(|pending| answer.tools.contains(pending))
            && self.refresh_cmd.as_deref().is_none_or(|command| answer.refresh_cmd == Some(command))
    }
}

/// What `record` does not cover of a run in the project at `root` that starts
/// `tools` and, with `refresh_cmd`, the custom command. Starts no process.
pub fn pending(
    record: &CoverageConsent,
    root: &Path,
    tools: &[&str],
    refresh_cmd: Option<&str>,
) -> PendingConsent {
    let consented = record.tools(root);
    let mut pending = PendingConsent {
        tools: tools
            .iter()
            .filter(|t| !consented.contains(**t))
            .map(|t| (*t).to_string())
            .collect(),
        ..PendingConsent::default()
    };
    if let Some(command) = refresh_cmd {
        let pinned = record.refresh_cmd_sha256(root);
        let named = consented.contains(CUSTOM_TOOL);
        if !named || pinned != Some(refresh_cmd_sha256(command).as_str()) {
            pending.command_changed = named && pinned.is_some();
            pending.refresh_cmd = Some(command.to_string());
            pending.tools.push(CUSTOM_TOOL.to_string());
        }
    }
    pending
}

/// Check the project's consent for a run that starts `tools` and, with
/// `refresh_cmd`, the custom command, before any process starts; `entry` is
/// the command a terminal user runs with `--consent` to give it. Returns the
/// tools this call recorded: empty when the consent already covered the run.
///
/// # Errors
/// A [`ConfigError::InvalidValue`] (exit 2) carrying the consent statement
/// when consent is missing and `consent` is false — nothing is recorded then;
/// else as [`load_coverage_consent`] and [`record_coverage_consent`].
pub fn require(
    user_dir: Option<&Path>,
    root: &Path,
    tools: &[&str],
    refresh_cmd: Option<&str>,
    consent: bool,
    entry: &str,
) -> Result<Vec<String>, ConfigError> {
    let pending = pending(&load_coverage_consent(user_dir)?, root, tools, refresh_cmd);
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    if !consent {
        return Err(refused(&pending, entry));
    }
    record(user_dir, root, &pending)?;
    Ok(pending.tools)
}

/// Record exactly what `pending` names in the project's consent, the custom
/// command pinned by the hash of its text: the one way a consent is recorded,
/// from a terminal's `--consent` and from the web UI's consent dialog alike.
///
/// # Errors
/// As [`record_coverage_consent`].
pub fn record(user_dir: Option<&Path>, root: &Path, pending: &PendingConsent) -> Result<(), ConfigError> {
    let hash = pending.refresh_cmd.as_deref().map(refresh_cmd_sha256);
    record_coverage_consent(user_dir, root, &pending.tools, hash.as_deref())?;
    Ok(())
}

/// The error a run the consent does not cover is refused with (exit 2): the
/// consent statement for `pending`, naming `<entry> --consent`.
pub fn refused(pending: &PendingConsent, entry: &str) -> ConfigError {
    ConfigError::InvalidValue {
        key: "consent".to_string(),
        message: refusal(pending, entry),
    }
}

/// The consent statement a refused run prints: what is not covered, what a
/// run does, the custom command verbatim, and how to give the consent.
fn refusal(pending: &PendingConsent, entry: &str) -> String {
    let changed = "the [coverage_ingest].refresh_cmd text changed since it was consented to in this \
                   project";
    // A changed command is said to have changed, never to have no consent.
    let unnamed: Vec<&str> = pending
        .tools
        .iter()
        .map(String::as_str)
        .filter(|t| !(pending.command_changed && *t == CUSTOM_TOOL))
        .collect();
    let what = match (unnamed.is_empty(), pending.command_changed) {
        (true, _) => changed.to_string(),
        (false, false) => format!("no coverage execution consent is recorded in this project for {}", unnamed.join(", ")),
        (false, true) => format!(
            "no coverage execution consent is recorded in this project for {}, and {changed}",
            unnamed.join(", ")
        ),
    };
    let command = pending.refresh_cmd.as_deref().map_or(String::new(), |c| {
        format!(" The custom command it runs is, verbatim:\n{}\n", shown(c))
    });
    format!(
        "{what} — a coverage run builds this project and runs its tests, which executes the \
         project's own code, and it may reach the network to fetch dependencies.{command} Run \
         `{entry} --consent` in a terminal to record it (an MCP client cannot); later runs will \
         not ask again until another tool is enabled or the command changes"
    )
}

/// `command` as a consent statement shows it: each character as it is, except
/// a control or bidirectional-formatting character other than a newline or a
/// tab, which is shown as its escape (`\u{1b}`). The committed text is TOML,
/// whose escapes decode to such characters, and a terminal would act on them —
/// a carriage return and a line erase can make it display a command other
/// than the one that runs, and the one the consent then pins.
pub fn shown(command: &str) -> String {
    let hidden = |c: char| {
        (c.is_control() && c != '\n' && c != '\t')
            || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    command
        .chars()
        .map(|c| if hidden(c) { c.escape_unicode().to_string() } else { c.to_string() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_scan_consent, record_scan_consent, COVERAGE_CONSENT_FILE, SCAN_CONSENT_FILE};

    #[test]
    fn a_command_is_shown_with_its_terminal_controls_escaped() {
        let (user, root) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        let disguised = "fetch-and-run #\r\u{1b}[2Kcargo llvm-cov\u{202e}x";
        let message = refused(user.path(), root.path(), &[], Some(disguised));
        assert!(
            message.contains("verbatim:\nfetch-and-run #\\u{d}\\u{1b}[2Kcargo llvm-cov\\u{202e}x\n"),
            "{message:?}"
        );
        assert!(!message.contains(['\r', '\u{1b}', '\u{202e}']), "{message:?}");
        assert_eq!(shown("make cov\n\tcheck"), "make cov\n\tcheck", "a newline and a tab are shown as they are");
    }

    const ENTRY: &str = "logos coverage refresh";

    fn refused(user: &Path, root: &Path, tools: &[&str], cmd: Option<&str>) -> String {
        match require(Some(user), root, tools, cmd, false, ENTRY) {
            Err(ConfigError::InvalidValue { key, message }) if key == "consent" => message,
            other => panic!("expected a consent refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_hash_pins_the_exact_text() {
        assert_eq!(
            refresh_cmd_sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(refresh_cmd_sha256("make cov"), refresh_cmd_sha256("make cov "));
    }

    #[test]
    fn a_run_without_consent_refuses_naming_the_flag_and_records_nothing() {
        let (user, root) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        let message = refused(user.path(), root.path(), &[], Some("make cov"));
        assert!(message.contains("`logos coverage refresh --consent`"), "{message}");
        assert!(message.contains("for custom —"), "{message}");
        assert!(message.contains("executes the project's own code"), "{message}");
        assert!(message.contains("may reach the network"), "{message}");
        assert!(message.contains("an MCP client cannot"), "{message}");
        assert!(message.contains("verbatim:\nmake cov\n"), "{message}");
        assert_eq!(std::fs::read_dir(user.path()).unwrap().count(), 0, "a refusal records nothing");
    }

    #[test]
    fn consent_is_asked_once_and_a_newly_enabled_tool_is_asked_for_alone() {
        let (user, root) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        let (user, root) = (user.path(), root.path());
        let message = refused(user, root, &["cargo-llvm-cov"], Some("make cov"));
        assert!(message.contains("for cargo-llvm-cov, custom —"), "every tool is named: {message}");
        let first = require(Some(user), root, &["cargo-llvm-cov"], Some("make cov"), true, ENTRY).unwrap();
        assert_eq!(first, ["cargo-llvm-cov", CUSTOM_TOOL]);
        let again = require(Some(user), root, &["cargo-llvm-cov"], Some("make cov"), false, ENTRY);
        assert_eq!(again.unwrap(), Vec::<String>::new(), "consent is not asked twice");

        let message = refused(user, root, &["cargo-llvm-cov", "vitest"], Some("make cov"));
        assert!(message.contains("for vitest —"), "only the new tool is asked for: {message}");
        assert!(!message.contains("verbatim"), "the unchanged command is not asked for: {message}");
        let added = require(Some(user), root, &["cargo-llvm-cov", "vitest"], Some("make cov"), true, ENTRY);
        assert_eq!(added.unwrap(), ["vitest"]);
        let record = load_coverage_consent(Some(user)).unwrap();
        assert_eq!(record.refresh_cmd_sha256(root), Some(refresh_cmd_sha256("make cov").as_str()));
    }

    #[test]
    fn a_changed_command_is_asked_for_again_showing_the_new_text_verbatim() {
        let (user, root) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        let (user, root) = (user.path(), root.path());
        require(Some(user), root, &[], Some("make cov"), true, ENTRY).unwrap();
        let changed = "make cov  COVERAGE_FLAGS='--all'";
        let message = refused(user, root, &[], Some(changed));
        assert!(message.starts_with("the [coverage_ingest].refresh_cmd text changed"), "{message}");
        assert!(message.contains(&format!("verbatim:\n{changed}\n")), "{message}");
        assert!(!message.contains("make cov\n"), "the old text is not shown as the command: {message}");
        // With a new tool too, both are said: the tool is new, the command changed.
        let mixed = refused(user, root, &["vitest"], Some(changed));
        assert!(
            mixed.starts_with(
                "no coverage execution consent is recorded in this project for vitest, and the \
                 [coverage_ingest].refresh_cmd text changed"
            ),
            "{mixed}"
        );
        assert!(mixed.contains(&format!("verbatim:\n{changed}\n")), "{mixed}");
        assert_eq!(require(Some(user), root, &[], Some(changed), true, ENTRY).unwrap(), [CUSTOM_TOOL]);
        assert!(require(Some(user), root, &[], Some(changed), false, ENTRY).unwrap().is_empty());
        // The earlier text is no longer covered: one pin per project.
        refused(user, root, &[], Some("make cov"));
        // A run that does not start the custom command does not check its pin.
        assert!(require(Some(user), root, &[], None, false, ENTRY).unwrap().is_empty());
    }

    #[test]
    fn the_consent_is_per_project_and_leaves_the_security_consent_alone() {
        let user = tempfile::TempDir::new().unwrap();
        let (a, b) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        record_scan_consent(Some(user.path()), a.path(), &["semgrep"]).unwrap();
        let scan_before = std::fs::read(user.path().join(SCAN_CONSENT_FILE)).unwrap();
        require(Some(user.path()), a.path(), &[], Some("make cov"), true, ENTRY).unwrap();
        refused(user.path(), b.path(), &[], Some("make cov"));
        assert_eq!(std::fs::read(user.path().join(SCAN_CONSENT_FILE)).unwrap(), scan_before);
        assert!(user.path().join(COVERAGE_CONSENT_FILE).is_file());
        assert_eq!(load_scan_consent(Some(user.path())).unwrap().consented(b.path()).len(), 0);
        for project in [a.path(), b.path()] {
            assert_eq!(std::fs::read_dir(project).unwrap().count(), 0, "nothing inside a project");
        }
    }
}
