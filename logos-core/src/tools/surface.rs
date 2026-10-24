//! What `logos security install|uninstall` and `security scan --install`
//! return ([FR-SC-22], S-744): the [`Installer`] behind a per-user,
//! per-tool-version consent, deciding what to install by the same precedence
//! detection uses.
//!
//! # What is installed
//! A tool is installed only when no usable copy is there: no user-level
//! override, no `PATH` copy at or above the manifest's minimum version, and
//! no managed copy ([`Orchestrator::copy_in_use`]). Every other tool is
//! reported as [kept](KeptTool), with its copy and version, and is never
//! fetched. `--update` also replaces a managed copy older than the pin.
//!
//! # Consent
//! Before anything is fetched, each tool to install must be covered by the
//! user's recorded [install consent](crate::config::InstallConsent) for its
//! pinned version. Without it, and without `consent`, the call fails with exit
//! 2 naming each tool, version, source and size and the command that records
//! it, and nothing is downloaded. With `consent`, the missing lines are
//! recorded first. The consent covers that version in every project; a newly
//! pinned version is asked for again. A Python scanner's line names the
//! package index, and `uv` and the managed Python it needs are lines of their
//! own while they are not installed (S-745). `--from <dir>` installs through the same
//! consent and the same verification, from the directory instead of the
//! network.
//!
//! # Opt-in tools
//! Dependency-Check and CodeQL are never part of `--all` or of `scan
//! --install`: each is installed only when named ([FR-SC-23], S-746).
//! Dependency-Check brings the managed Java runtime, a consent line of its own
//! while it is missing. CodeQL is refused, with its licence terms, until its
//! licence is accepted (`--accept-codeql-licence`); the acknowledgement is
//! recorded per version, with the install consent, once the install goes
//! ahead.
//!
//! # Uninstall
//! Removes the managed copies only ([`Installer::uninstall`]); the copies the
//! user installed are reported as kept, untouched. `--all` covers the opt-in
//! tools too, and the Java runtime goes once no installed tool needs it.
//!
//! # The Security page
//! The page's **Install**, **Update** and **Install & scan** ([FR-SC-24],
//! S-747) go through [`admit_install`]: it runs them only once the page's one
//! consent dialog has answered every line they need, and otherwise returns the
//! [plan](InstallPlan) the dialog shows — each tool, support install and
//! pending licence acknowledgement a line of its own, the opt-in tools offered
//! unticked. It decides on recorded state only and starts no process. The
//! CLI keeps its licence refusal. [`install_job`] is the
//! `tool-install` job's work: the install, then the scan, cancellable.
//!
//! # The coverage install
//! [`coverage_install`] is `logos coverage install` ([FR-CV-17], S-748): the
//! managed [coverage tools](Manifest::coverage) — `cargo-llvm-cov` — by the
//! same precedence, consent and verified route, and the `llvm-tools-preview`
//! component added by the user's own `rustup`. The security surfaces above
//! never list, install or remove a coverage tool. The Coverage page's
//! **Install** ([FR-CV-17], S-750) asks for the same lines first:
//! [`coverage_install_pending`] lists those the consent does not cover yet,
//! starting no process, and [`record_coverage_install_lines`] records the ones
//! its dialog accepted; the `coverage-run` job then installs through
//! [`coverage_install_reporting`].
//!
//! No MCP tool calls into this module: an agent cannot grant itself consent
//! to download and run a binary ([ADR-88] §6).
//!
//! [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
//! [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
//! [FR-SC-22]: ../../../docs/specs/requirements/FR-SC-22.md
//! [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
//! [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::{
    ComponentOutcome, InstallOutcome, InstallProgress, Installer, Manifest, Platform, Source, ToolEntry,
    ToolInstall, ToolUninstall, LLVM_TOOLS, LLVM_TOOLS_ADD,
};
use crate::config::{
    load_install_consent, load_scan_consent, load_toolchain_consent, record_install_consent,
    record_toolchain_consent, ConfigError,
};
use crate::coverage::{CoverageRunner, Detection, Origin, Probes};
use crate::jobs::spawn::inside_root;
use crate::security::{
    consent_pending, Availability, CopyKind, InstalledCopy, Orchestrator, Probe, ScanEnvironment,
    ScanProgress, Selection, Unprobed,
};

/// What `logos security install` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRequest {
    /// The tools, by registry name, or every tool of the default set — never
    /// an [opt-in](super::OptIn) one.
    pub tools: Selection,
    /// Also replace a managed copy older than the manifest's pin.
    pub update: bool,
    /// Install from this directory of pre-downloaded artifacts instead of the
    /// network.
    pub from: Option<PathBuf>,
    /// Record the install consent for what is about to be installed.
    pub consent: bool,
    /// The tools whose licence the user accepts with this call
    /// (`--accept-codeql-licence` names `codeql`).
    pub accept_licences: Vec<String>,
}

/// What an install returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityInstall {
    /// The `<tool> <version>` lines this call added to the install consent.
    pub consent_recorded: Vec<String>,
    /// The `<tool> <version>` licences this call recorded as accepted.
    pub licence_recorded: Vec<String>,
    /// The tools left as they are, because a usable copy is already there.
    pub kept: Vec<KeptTool>,
    /// One entry per tool installed or attempted, in manifest order.
    pub tools: Vec<ToolInstall>,
}

impl SecurityInstall {
    /// `logos security install`'s exit code: 1 when the call attempted tools
    /// and none of what it was asked for is in place — nothing ended
    /// `installed` or `already_present`, and no usable copy was kept — else 0,
    /// however many of the others failed (each tool's outcome says which).
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let in_place = !self.kept.is_empty()
            || self.tools.iter().any(|t| {
                matches!(
                    t.outcome,
                    InstallOutcome::Installed { .. } | InstallOutcome::AlreadyPresent { .. }
                )
            });
        i32::from(!self.tools.is_empty() && !in_place)
    }
}

/// A tool an install or uninstall left in place, and the copy it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptTool {
    pub tool: String,
    #[serde(flatten)]
    pub copy: InstalledCopy,
}

/// What an uninstall returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityUninstall {
    /// One entry per tool asked for: its managed copies removed or not.
    pub tools: Vec<ToolUninstall>,
    /// The copies still there afterwards — the user's own, never touched.
    pub kept: Vec<KeptTool>,
}

/// What `security scan --install` returns: the install, then the scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityScanInstall {
    pub install: SecurityInstall,
    pub scan: crate::security::SecurityScanSummary,
}

/// One tool an install would fetch, as the consent names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Planned {
    tool: String,
    version: String,
    /// The download host, or the `--from` directory.
    source: String,
    size: u64,
    /// For a support line (`uv`, the managed Python, the Java runtime): the
    /// planned tools that need it. Empty for a tool's own line.
    for_tools: Vec<String>,
}

/// Install what `request` names and is missing, under the install consent the
/// [module docs](self) describe. `root` is the project the command runs in:
/// its `PATH` entries are never used to find a copy, `curl` or `tar`.
///
/// # Errors
/// A [`ConfigError`] (exit 2): a tool the manifest does not list, a platform
/// without managed installs, no tools directory or one inside the project, an
/// invalid config, or a consent that is required and not given — each before
/// anything is fetched. One tool's failure is its `failed` outcome.
pub fn install(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    request: &InstallRequest,
) -> Result<SecurityInstall> {
    install_reporting(root, env, manifest, request, &mut |_| {})
}

/// [`install`], handing each tool's start and outcome to `progress`; on a
/// [job](crate::jobs)'s thread a cancel stops it ([`Installer::install_with`]).
///
/// # Errors
/// As [`install`].
fn install_reporting(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    request: &InstallRequest,
    progress: &mut dyn FnMut(&InstallProgress),
) -> Result<SecurityInstall> {
    let names = selected(manifest, &request.tools, false)?;
    let flags: Vec<&str> = names
        .iter()
        .filter_map(|name| Some(manifest.tool(name)?.licence()?.flag))
        .collect();
    let retry = retry_command(request, &flags);
    install_selected(root, env, manifest, &names, request, &retry, progress)
}

/// `security scan --install`'s install half: every tool of the manifest's
/// default set that this project has not disabled — never an opt-in one —
/// installed as [`install`] would, with
/// `logos security scan --install --consent` named as the consenting command.
///
/// # Errors
/// As [`install`].
pub fn install_for_scan(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    consent: bool,
) -> Result<SecurityInstall> {
    let enabled = enabled_tools(root, env, manifest, ToolEntry::is_default)?;
    let request = InstallRequest {
        tools: Selection::Only(enabled.clone()),
        update: false,
        from: None,
        consent,
        accept_licences: Vec::new(),
    };
    install_selected(
        root,
        env,
        manifest,
        &enabled,
        &request,
        "logos security scan --install --consent",
        &mut |_| {},
    )
}

/// The manifest tools `select` picks that this project has not disabled, in
/// registry order: Install & scan's default set, or its opt-in offer.
fn enabled_tools(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    select: fn(&ToolEntry) -> bool,
) -> Result<Vec<String>> {
    Ok(Orchestrator::open_in(root, env)?
        .availability()
        .into_iter()
        .filter(|(s, a)| *a != Availability::Disabled && manifest.tool(s.name).is_some_and(select))
        .map(|(s, _)| s.name.to_string())
        .collect())
}

/// `names` split into the tools left as they are — a usable copy is there —
/// and the tools to install, by the precedence detection uses, each copy's
/// version read as `probe` says.
fn partition(
    orchestrator: &Orchestrator,
    manifest: &Manifest,
    names: &[String],
    update: bool,
    probe: Probe,
) -> (Vec<KeptTool>, Vec<String>) {
    let mut kept = Vec::new();
    let mut planned = Vec::new();
    for name in names {
        // A set override is the user's choice even when it cannot run: a
        // managed copy installed beside it would never be used.
        let copy = orchestrator.copy_in_use_with(name, probe).or_else(|| {
            orchestrator.override_of(name).map(|path| InstalledCopy {
                copy: CopyKind::Override,
                binary: path.to_path_buf(),
                version: None,
                update_available: None,
                unprobed: None,
            })
        });
        if let Some(copy) = copy.filter(|c| keeps(manifest, name, c, update)) {
            kept.push(KeptTool {
                tool: name.clone(),
                copy,
            });
        } else {
            planned.push(name.clone());
        }
    }
    (kept, planned)
}

fn install_selected(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    names: &[String],
    request: &InstallRequest,
    retry: &str,
    progress: &mut dyn FnMut(&InstallProgress),
) -> Result<SecurityInstall> {
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    let source = match &request.from {
        Some(dir) => Source::Directory(dir.clone()),
        None => Source::Network,
    };

    let (kept, planned) = partition(&orchestrator, manifest, names, request.update, Probe::Run);
    if planned.is_empty() {
        // Nothing to install, so no tools directory or platform is needed.
        return Ok(SecurityInstall {
            consent_recorded: Vec::new(),
            licence_recorded: Vec::new(),
            kept,
            tools: Vec::new(),
        });
    }
    // Built before the consent check, so a tools directory inside the project
    // is refused before anything is recorded.
    let installer = installer(root, env, manifest)?;
    let lines = plan(&installer, &planned, &source);
    let consented = load_install_consent(env.user_dir.as_deref())?;
    // A licensed tool is refused, its terms shown, until its licence is
    // accepted — before the consent, which it does not replace.
    let mut licences = Vec::new();
    for name in &planned {
        let Some(entry) = manifest.tool(name) else {
            continue;
        };
        let Some(licence) = entry.licence() else {
            continue;
        };
        if consented.licence_accepted(name, &entry.version) {
            continue;
        }
        if !request.accept_licences.contains(name) {
            return Err(ConfigError::InvalidValue {
                key: "licence".to_string(),
                message: format!(
                    "{name} {} is installed only once its licence is accepted: {}. Nothing was \
                     downloaded. Run `{retry}` in a terminal to accept it ({} records the \
                     acknowledgement for this version; an MCP client cannot)",
                    entry.version, licence.terms, licence.flag
                ),
            }
            .into());
        }
        licences.push((name.as_str(), entry.version.as_str()));
    }
    let missing: Vec<&Planned> = lines
        .iter()
        .filter(|p| !consented.covers(&p.tool, &p.version))
        .collect();
    if !missing.is_empty() && !request.consent {
        return Err(ConfigError::InvalidValue {
            key: "consent".to_string(),
            message: format!(
                "no install consent is recorded for {}. Nothing was downloaded. Run `{retry}` in \
                 a terminal to record it (an MCP client cannot); it covers these versions in \
                 every project",
                missing
                    .iter()
                    .map(|p| describe(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
        .into());
    }
    if !missing.is_empty() || !licences.is_empty() {
        let pairs: Vec<(&str, &str)> = missing
            .iter()
            .map(|p| (p.tool.as_str(), p.version.as_str()))
            .collect();
        record_install_consent(env.user_dir.as_deref(), &pairs, &licences)?;
    }

    let report = installer.install_with(&planned, &source, request.update, progress)?;
    Ok(SecurityInstall {
        consent_recorded: missing
            .iter()
            .map(|p| format!("{} {}", p.tool, p.version))
            .collect(),
        licence_recorded: licences
            .iter()
            .map(|(tool, version)| format!("{tool} {version}"))
            .collect(),
        kept,
        tools: report.tools,
    })
}

/// Remove the managed copies of the selected tools, and report the copies the
/// user installed, which stay.
///
/// # Errors
/// As [`install`], but never for consent: removing needs none.
pub fn uninstall(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    tools: &Selection,
) -> Result<SecurityUninstall> {
    let names = selected(manifest, tools, true)?;
    let removed = installer(root, env, manifest)?.uninstall(&names)?;
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    let kept = names
        .iter()
        .filter_map(|name| {
            orchestrator.copy_in_use(name).map(|copy| KeptTool {
                tool: name.clone(),
                copy,
            })
        })
        .collect();
    Ok(SecurityUninstall {
        tools: removed,
        kept,
    })
}

// ── The Security page's install (S-747, FR-SC-24) ────────────────────────────

/// What the Security page asks to install ([FR-SC-24]): a row's **Install**
/// or **Update**, or **Install & scan**, with the answers its one consent
/// dialog gave. [`admit_install`] turns it into an [`InstallRequest`] once
/// every line the install and its scan need is answered.
///
/// [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallAsk {
    /// The tools a row names; `None` is Install & scan's: every tool of the
    /// default set this project has not disabled, with the opt-in tools
    /// offered beside them.
    pub tools: Option<Vec<String>>,
    /// Replace a managed copy older than the pin (a row's Update).
    pub update: bool,
    /// Run the full scan once the install is done (Install & scan).
    pub scan: bool,
    /// The offered opt-in tools the user ticked.
    pub opt_in: Vec<String>,
    /// The install-consent lines the dialog showed and the user accepted,
    /// each `<tool> <version>`.
    pub consent: Vec<String>,
    /// The tools whose licence line the user ticked.
    pub accept_licences: Vec<String>,
    /// The scanners whose network consent the dialog asked for and the user
    /// accepted ([FR-SC-07]).
    ///
    /// [FR-SC-07]: ../../../docs/specs/requirements/FR-SC-07.md
    pub scan_consent: Vec<String>,
}

/// One line of the Security page's consent dialog: what an install would
/// download, or a licence to acknowledge first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "line", rename_all = "snake_case")]
pub enum ConsentLine {
    /// A tool to install: its pinned version, source host and download size.
    Tool {
        tool: String,
        version: String,
        source: String,
        size: u64,
        /// The install consent already covers this version.
        recorded: bool,
        /// An opt-in tool Install & scan offers ([FR-SC-23]): installed only
        /// when its line is ticked.
        ///
        /// [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
        offered: bool,
    },
    /// A support install — `uv` and the managed Python, or the Java runtime —
    /// that the tools in `for_tools` need, while it is not installed.
    Support {
        tool: String,
        version: String,
        source: String,
        size: u64,
        recorded: bool,
        for_tools: Vec<String>,
    },
    /// A licence `tool` installs only once acknowledged (CodeQL, [FR-SC-23],
    /// [ADR-88] §6): a pending line of its own, while this version's
    /// acknowledgement is not recorded.
    ///
    /// [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
    /// [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md
    Licence {
        tool: String,
        version: String,
        terms: String,
        flag: String,
    },
}

/// What the consent dialog shows before an install: the tool installer's
/// `plan` ([tool-installer]).
///
/// [tool-installer]: ../../../docs/specs/architecture/components/tool-installer.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallPlan {
    /// Each tool's line, its support lines, then the licence lines.
    pub lines: Vec<ConsentLine>,
    /// The tools left as they are: a copy is already there. No version is
    /// probed, so each reads `version: null`, `unprobed: "state_only"`; a
    /// managed copy's `update_available` still comes from its directory.
    pub kept: Vec<KeptTool>,
    /// For Install & scan, the scanners this project's network consent does
    /// not name yet: those installed now, those about to be, and the offered
    /// opt-in tools — asked for only when ticked.
    pub scan_consent: Vec<String>,
}

/// What [`admit_install`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallAdmission {
    /// Every line is answered: record the project's network consent for
    /// `scan_consent`, then run `request` — and the scan, when `scan`.
    Ready {
        request: InstallRequest,
        scan: bool,
        scan_consent: Vec<String>,
    },
    /// A line the install or its scan needs is unanswered: the dialog shows
    /// this plan, and nothing is recorded or fetched.
    Ask(InstallPlan),
}

/// Decide whether the Security page's `ask` can run as it stands. Every line
/// it needs must be answered: each tool and support line not yet recorded
/// accepted by its `<tool> <version>`, each pending licence ticked, and —
/// for Install & scan — each scanner short of the project's network consent
/// accepted. An offered opt-in tool counts only when ticked. Otherwise the
/// answer is the [plan](InstallPlan) the dialog shows.
///
/// Decides on recorded state only (S-766): the copies there, the managed
/// versions, the consent records and the config. It starts no process — no
/// copy's version is probed — and writes nothing. A `PATH` copy with no
/// managed copy beside it is therefore [kept](KeptTool) whatever its version,
/// as the page shows it installed. Beside a managed copy, admission sees the
/// managed one, since only a version could prefer the `PATH` copy. The job,
/// once the dialog is answered, installs at most the tools admitted here: its
/// own probed partition can only keep more of them.
///
/// # Errors
/// A [`ConfigError`] (exit 2): an unknown tool, an `opt_in` tool that is not
/// offered, a platform without managed installs, no tools directory or one
/// inside the project, or an invalid config.
pub fn admit_install(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    ask: &InstallAsk,
) -> Result<InstallAdmission> {
    let (named, offered_names) = match &ask.tools {
        Some(tools) => (
            selected(manifest, &Selection::Only(tools.clone()), false)?,
            Vec::new(),
        ),
        None => (
            enabled_tools(root, env, manifest, ToolEntry::is_default)?,
            enabled_tools(root, env, manifest, |t| !t.is_default())?,
        ),
    };
    if let Some(bad) = ask.opt_in.iter().find(|t| !offered_names.contains(t)) {
        return Err(ConfigError::InvalidValue {
            key: "opt_in".to_string(),
            message: format!("{bad:?} is not an opt-in tool this install offers"),
        }
        .into());
    }
    let orchestrator = Orchestrator::open_in(root, env)?.with_manifest(manifest.clone());
    // Which copies are there, never what they print: nothing runs before the
    // dialog is answered.
    let state = Probe::Skip(Unprobed::StateOnly);
    let (kept, planned) = partition(&orchestrator, manifest, &named, ask.update, state);
    let (_, offered) = partition(&orchestrator, manifest, &offered_names, false, state);
    let chosen: Vec<String> = planned
        .iter()
        .chain(offered.iter().filter(|t| ask.opt_in.contains(t)))
        .cloned()
        .collect();

    let all: Vec<String> = planned.iter().chain(&offered).cloned().collect();
    let lines = consent_lines(root, env, manifest, &all, &offered)?;
    let scan_consent = if ask.scan {
        scan_consent_missing(root, env, manifest, &all)?
    } else {
        Vec::new()
    };

    // What this ask installs and scans: the offered tools only when ticked.
    let needed = |tool: &String| !offered.contains(tool) || chosen.contains(tool);
    let unanswered = lines.iter().any(|line| is_unanswered(line, ask, &chosen))
        || scan_consent
            .iter()
            .any(|s| needed(s) && !ask.scan_consent.contains(s));
    if unanswered {
        return Ok(InstallAdmission::Ask(InstallPlan {
            lines,
            kept,
            scan_consent,
        }));
    }
    Ok(InstallAdmission::Ready {
        request: InstallRequest {
            tools: Selection::Only(chosen.clone()),
            update: ask.update,
            from: None,
            // Every line not yet recorded was accepted just above, so the
            // install records exactly those.
            consent: true,
            accept_licences: ask
                .accept_licences
                .iter()
                .filter(|t| chosen.contains(t))
                .cloned()
                .collect(),
        },
        scan: ask.scan,
        scan_consent: scan_consent.into_iter().filter(|s| needed(s)).collect(),
    })
}

/// The consent dialog's lines for `all`, the tools [`admit_install`] plans:
/// each tool's line (an `offered` one marked so) and its support lines, in the
/// installer's plan order, then a licence line for each tool whose licence is
/// not yet acknowledged for its version. No line, and nothing read, when
/// nothing is planned.
fn consent_lines(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    all: &[String],
    offered: &[String],
) -> Result<Vec<ConsentLine>> {
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let installer = installer(root, env, manifest)?;
    let consented = load_install_consent(env.user_dir.as_deref())?;
    let mut lines: Vec<ConsentLine> = plan(&installer, all, &Source::Network)
        .into_iter()
        .map(|p| {
            let recorded = consented.covers(&p.tool, &p.version);
            planned_line(p, recorded, offered)
        })
        .collect();
    lines.extend(all.iter().filter_map(|name| {
        let entry = manifest.tool(name)?;
        let licence = entry.licence()?;
        (!consented.licence_accepted(name, &entry.version)).then(|| ConsentLine::Licence {
            tool: name.clone(),
            version: entry.version.clone(),
            terms: licence.terms.to_string(),
            flag: licence.flag.to_string(),
        })
    }));
    Ok(lines)
}

/// A planned install's dialog line: a support line when it serves other
/// tools, else the tool's own line, marked when it is an `offered` opt-in.
fn planned_line(p: Planned, recorded: bool, offered: &[String]) -> ConsentLine {
    if p.for_tools.is_empty() {
        return ConsentLine::Tool {
            offered: offered.contains(&p.tool),
            tool: p.tool,
            version: p.version,
            source: p.source,
            size: p.size,
            recorded,
        };
    }
    ConsentLine::Support {
        tool: p.tool,
        version: p.version,
        source: p.source,
        size: p.size,
        recorded,
        for_tools: p.for_tools,
    }
}

/// For Install & scan, the scanners the project's network consent does not
/// name: those already installed, then those in `all`, each once.
fn scan_consent_missing(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    all: &[String],
) -> Result<Vec<String>> {
    let consented = load_scan_consent(env.user_dir.as_deref())?.consented(root);
    let installed = consent_pending(root, env, manifest)?;
    let mut missing: Vec<String> = Vec::new();
    for name in installed.iter().map(|s| s.to_string()).chain(all.iter().cloned()) {
        if !consented.contains(&name) && !missing.contains(&name) {
            missing.push(name);
        }
    }
    Ok(missing)
}

/// Whether `ask` leaves `line` unanswered while it needs it: a tool or
/// support line of a `chosen` tool that is neither recorded nor accepted by
/// its `<tool> <version>`, or a chosen tool's licence left unticked.
fn is_unanswered(line: &ConsentLine, ask: &InstallAsk, chosen: &[String]) -> bool {
    let accepted = |tool: &str, version: &str| ask.consent.contains(&format!("{tool} {version}"));
    match line {
        ConsentLine::Tool { tool, version, recorded, .. } => {
            chosen.contains(tool) && !recorded && !accepted(tool, version)
        }
        ConsentLine::Support { tool, version, recorded, for_tools, .. } => {
            for_tools.iter().any(|t| chosen.contains(t)) && !recorded && !accepted(tool, version)
        }
        ConsentLine::Licence { tool, .. } => {
            chosen.contains(tool) && !ask.accept_licences.contains(tool)
        }
    }
}

/// One step of the Security page's install job, as it happens: a tool's
/// install, a scanner's run in the scan that follows, or — once the job is
/// cancelled — `{"phase": "cancelled", "installed": [...]}`, the tools that
/// landed before the cancel.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum InstallStep<'a> {
    Install(&'a InstallProgress),
    Scan(&'a ScanProgress),
    Cancelled {
        phase: &'static str,
        installed: &'a [String],
    },
}

/// What the Security page's install job returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityInstallJob {
    pub install: SecurityInstall,
    /// The scan that followed (Install & scan); `None` for an install alone.
    pub scan: Option<crate::security::SecurityScanSummary>,
}

/// A scan run with its progress: the engine's, so the reports are ingested.
pub type ScanRun<'r> = &'r mut dyn FnMut(
    &mut dyn FnMut(&ScanProgress),
) -> Result<crate::security::SecurityScanSummary>;

/// The `tool-install` job's work ([FR-SC-24]): install what `request` names,
/// as [`install`] does, then run `scan` when given — each step handed to
/// `progress`. On a [job](crate::jobs)'s thread, a cancel kills the download,
/// unpack or scanner running and starts nothing after it: no partial tool is
/// left, the last step names the tools installed by then, and the error is
/// [`Cancelled`](crate::jobs::Cancelled).
///
/// # Errors
/// As [`install`], and as the scan; `Cancelled` when the job was cancelled.
///
/// [FR-SC-24]: ../../../docs/specs/requirements/FR-SC-24.md
pub fn install_job(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    request: &InstallRequest,
    scan: Option<ScanRun<'_>>,
    progress: &mut dyn FnMut(&InstallStep<'_>),
) -> Result<SecurityInstallJob> {
    let install = install_reporting(root, env, manifest, request, &mut |step| {
        progress(&InstallStep::Install(step));
    })?;
    let installed: Vec<String> = install
        .tools
        .iter()
        .filter(|t| matches!(t.outcome, super::InstallOutcome::Installed { .. }))
        .map(|t| format!("{} {}", t.tool, t.version))
        .collect();
    let cancelled = |progress: &mut dyn FnMut(&InstallStep<'_>)| {
        progress(&InstallStep::Cancelled {
            phase: "cancelled",
            installed: &installed,
        });
    };
    if crate::jobs::cancelled() {
        cancelled(progress);
        return Err(crate::jobs::Cancelled.into());
    }
    let scan = match scan {
        None => None,
        Some(run) => match run(&mut |step| progress(&InstallStep::Scan(step))) {
            Ok(summary) => Some(summary),
            Err(e) => {
                if crate::jobs::cancelled() {
                    cancelled(progress);
                }
                return Err(e);
            }
        },
    };
    Ok(SecurityInstallJob { install, scan })
}

/// Whether a found copy stays: any override, a `PATH` copy at or above the
/// minimum, a managed copy — unless `update` asks for the pinned one. A `PATH`
/// copy whose version was not probed stays: it is the user's own, and the
/// Security page shows it installed.
fn keeps(manifest: &Manifest, tool: &str, copy: &InstalledCopy, update: bool) -> bool {
    match copy.copy {
        CopyKind::Override => true,
        CopyKind::Path => {
            copy.unprobed.is_some() || manifest.meets_minimum(tool, copy.version.as_deref())
        }
        CopyKind::Managed => !(update && copy.update_available.is_some()),
    }
}

/// The selected tool names, each one the manifest lists, in manifest order.
/// [`Selection::All`] is the default set, and the opt-in tools as well only
/// `with_opt_in` (an uninstall: removing asks for no opt-in).
fn selected(
    manifest: &Manifest,
    tools: &Selection,
    with_opt_in: bool,
) -> Result<Vec<String>, ConfigError> {
    let known = || {
        manifest
            .tools
            .iter()
            .map(|t| t.tool.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match tools {
        Selection::All => Ok(manifest
            .tools
            .iter()
            .filter(|t| with_opt_in || t.is_default())
            .map(|t| t.tool.clone())
            .collect()),
        Selection::Only(names) => {
            if let Some(bad) = names.iter().find(|n| manifest.tool(n).is_none()) {
                return Err(ConfigError::InvalidValue {
                    key: "tool".to_string(),
                    message: format!(
                        "{bad:?} is not a tool Logos installs (expected one of: {}, or --all)",
                        known()
                    ),
                });
            }
            Ok(manifest
                .tools
                .iter()
                .filter(|t| names.contains(&t.tool))
                .map(|t| t.tool.clone())
                .collect())
        }
    }
}

/// The installer for `env`, run for the project at `root` ([S-743] review: a
/// project's `PATH` entries never supply `curl` or `tar`).
///
/// [S-743]: ../../../docs/planning/journal.md#s-743-a-managed-tools-directory-installs-pinned-checksum-verified-scanner-binaries
fn installer(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
) -> Result<Installer, ConfigError> {
    let installer = Installer::from_env(env.tools_dir.clone(), Platform::current())?;
    let canonical_root = root.canonicalize().ok();
    if inside_root(root, canonical_root.as_deref(), installer.tools_dir()) {
        return Err(ConfigError::InvalidValue {
            key: "tools".to_string(),
            message: format!(
                "the tools directory {} is inside the project; Logos never installs a tool into \
                 a repository",
                installer.tools_dir().display()
            ),
        });
    }
    Ok(installer
        .with_manifest(manifest.clone())
        .with_search_path(env.search_path.clone())
        .for_project(root))
}

/// What the consent names for each tool to install: its pinned version, where
/// it comes from and how large it is. A tool with no build for this platform
/// downloads nothing, so it is not asked for. A Python tool comes from the
/// package index, and the first one also names `uv` and the managed Python,
/// each while it is not installed yet (S-745); a Java tool names the managed
/// Java runtime likewise (S-746).
fn plan(installer: &Installer, tools: &[String], source: &Source) -> Vec<Planned> {
    let Some(platform) = Platform::current() else {
        return Vec::new();
    };
    let manifest = installer.manifest();
    let from = |host: &str| match source {
        Source::Network => host.to_string(),
        Source::Directory(dir) => dir.display().to_string(),
    };
    let mut lines: Vec<Planned> = tools
        .iter()
        .filter_map(|tool| {
            let entry = manifest.entry(tool)?;
            let host = match &entry.python {
                Some(_) => manifest.python.index_host(),
                None => entry.artifact(platform)?.host(),
            };
            Some(Planned {
                tool: entry.tool.clone(),
                version: entry.version.clone(),
                source: from(host),
                size: entry.download_size(platform)?,
                for_tools: Vec::new(),
            })
        })
        .collect();
    // The support lines, in the order they are installed: `uv` and the
    // managed Python, then the Java runtime — each naming the tools it is for.
    let needing = |need: fn(&ToolEntry) -> bool| -> Vec<String> {
        lines
            .iter()
            .filter(|l| manifest.tool(&l.tool).is_some_and(need))
            .map(|l| l.tool.clone())
            .collect()
    };
    let mut support = Vec::new();
    let python = needing(|e| e.python.is_some());
    if !python.is_empty() {
        support.extend(
            installer
                .python_support_pending(platform)
                .into_iter()
                .map(|line| (line, python.clone())),
        );
    }
    let java = needing(ToolEntry::needs_java);
    if !java.is_empty() {
        support.extend(installer.java_pending(platform).into_iter().map(|line| (line, java.clone())));
    }
    lines.extend(
        support
            .into_iter()
            .map(|((tool, version, host, size), for_tools)| Planned {
                tool,
                version,
                source: from(&host),
                size,
                for_tools,
            }),
    );
    lines
}

/// `trivy 0.74.0 from github.com (47.7 MB)`.
fn describe(p: &Planned) -> String {
    format!(
        "{} {} from {} ({:.1} MB)",
        p.tool,
        p.version,
        p.source,
        p.size as f64 / 1_000_000.0
    )
}

/// The command that repeats `request` with the consent given, and each licence
/// `flag` (the selected licensed tools') with it.
fn retry_command(request: &InstallRequest, flags: &[&str]) -> String {
    let mut command = "logos security install".to_string();
    match &request.tools {
        Selection::All => command.push_str(" --all"),
        Selection::Only(names) => {
            for name in names {
                command.push(' ');
                command.push_str(name);
            }
        }
    }
    if request.update {
        command.push_str(" --update");
    }
    if let Some(dir) = &request.from {
        command.push_str(&format!(" --from {}", shell_word(&dir.to_string_lossy())));
    }
    for flag in flags {
        command.push(' ');
        command.push_str(flag);
    }
    command.push_str(" --consent");
    command
}

/// `word` as one shell word, so the named command runs as printed: as is
/// when it holds only characters no shell treats specially, else in single
/// quotes.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+:=@,".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

// ── The coverage install (S-748, FR-CV-17) ───────────────────────────────────

/// The command that installs the managed coverage tools, which a refusal
/// names with `--consent`.
pub const COVERAGE_INSTALL_ENTRY: &str = "logos coverage install";

/// What `logos coverage install` asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageInstallRequest {
    /// Install from this directory of pre-downloaded artifacts instead of the
    /// network.
    pub from: Option<PathBuf>,
    /// Record the install consent — and the toolchain consent for
    /// `rustup component add llvm-tools-preview` — for what is about to run.
    pub consent: bool,
}

/// What `logos coverage install` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageInstall {
    /// The lines this call added to the consent: `<tool> <version>`, and
    /// [`LLVM_TOOLS_ADD`].
    pub consent_recorded: Vec<String>,
    /// The coverage tools left as they are: a usable copy is already there.
    pub kept: Vec<KeptCoverageTool>,
    /// One entry per coverage tool installed or attempted.
    pub tools: Vec<ToolInstall>,
    /// The `llvm-tools-preview` step.
    pub llvm_tools: ComponentOutcome,
}

impl CoverageInstall {
    /// `logos coverage install`'s exit code: 1 when a coverage tool it set
    /// out to install is not in place, or when adding `llvm-tools-preview`
    /// failed or was cancelled; else 0. A kept copy counts as in place
    /// whether or not it can run: an override that cannot is the user's
    /// choice, and `coverage status` reports it. Without `rustup` the step changes
    /// nothing and names the command to run by hand, which is not a failure:
    /// a toolchain `rustup` does not manage brings its LLVM tools its own way.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        let tool_missing = self.tools.iter().any(|t| {
            !matches!(t.outcome, InstallOutcome::Installed { .. } | InstallOutcome::AlreadyPresent { .. })
        });
        let step_failed = matches!(self.llvm_tools, ComponentOutcome::Failed { .. } | ComponentOutcome::Cancelled);
        i32::from(tool_missing || step_failed)
    }
}

/// A coverage tool the install left in place, and the copy it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptCoverageTool {
    pub tool: String,
    /// `override`, `path` or `managed`.
    pub origin: Origin,
    pub binary: PathBuf,
    /// The version its probe printed (a managed copy's, its directory's);
    /// `None` when it could not be read.
    pub version: Option<String>,
}

/// Install the managed coverage tools that are missing, and add
/// `llvm-tools-preview` to the user's toolchain, under the consent the
/// [module docs](self) describe ([FR-CV-17]).
///
/// A tool is installed only when no usable copy is there: the user-level
/// `path` override (even one that cannot run: it is the user's choice), a
/// `PATH` copy at or above the manifest's minimum, or a managed copy. Before
/// anything is fetched or any `rustup` runs, every line must be consented to:
/// each tool to install by its `<tool> <version>` in the install consent, and
/// `rustup component add llvm-tools-preview` — while a `rustup` is on the
/// search path — in the toolchain consent. Without them, and without
/// `consent`, the call fails naming each line and the command that records it.
/// Without `rustup` the component step runs nothing and reports the command to
/// run by hand.
///
/// # Errors
/// A [`ConfigError`] (exit 2), before anything is fetched or run: an invalid
/// config; when a tool is to be installed, a platform without managed
/// installs, no tools directory or one inside the project; or a consent that
/// is required and not given.
///
/// [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
pub fn coverage_install(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    request: &CoverageInstallRequest,
) -> Result<CoverageInstall> {
    coverage_install_reporting(root, env, manifest, request, &mut |_| {})
}

/// One step of a coverage install as it happens, for the `coverage-run` job
/// that installs before it runs (S-750): a tool's install starting and
/// settled, as the Security page's install reports them, then the
/// `llvm-tools-preview` step. Serialised with a `phase` tag.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CoverageInstallStep<'a> {
    Install(&'a InstallProgress),
    Component(ComponentStep<'a>),
}

/// The `llvm-tools-preview` step of a coverage install, as it happens.
#[derive(Debug, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ComponentStep<'a> {
    /// `rustup` is about to list, and add if missing, the component.
    ComponentStarted { component: &'static str, command: &'static str },
    /// How the step ended.
    ComponentFinished {
        component: &'static str,
        #[serde(flatten)]
        outcome: &'a ComponentOutcome,
    },
}

/// [`coverage_install`], handing each step to `progress` as it happens; on a
/// [job](crate::jobs)'s thread a cancel kills the download, unpack or `rustup`
/// running, leaves no partial tool and starts nothing after it.
///
/// # Errors
/// As [`coverage_install`].
pub fn coverage_install_reporting(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
    request: &CoverageInstallRequest,
    progress: &mut dyn FnMut(&CoverageInstallStep<'_>),
) -> Result<CoverageInstall> {
    let runner = CoverageRunner::open_with(root, env.user_dir.as_deref(), env.search_path.clone())?
        .with_tools_dir(env.tools_dir.clone());
    let source = match &request.from {
        Some(dir) => Source::Directory(dir.clone()),
        None => Source::Network,
    };
    let mut kept = Vec::new();
    let mut planned = Vec::new();
    for entry in &manifest.coverage {
        match kept_coverage_copy(&runner, manifest, &entry.tool, Probes::Every) {
            Some(copy) => kept.push(copy),
            None => planned.push(entry.tool.clone()),
        }
    }
    // Built before the consent check, so a tools directory inside the project
    // is refused before anything is recorded. Adding the component writes no
    // tools directory, so it needs none.
    let installer = if planned.is_empty() {
        toolchain_installer(root, env, manifest)
    } else {
        installer(root, env, manifest)?
    };
    let lines = plan(&installer, &planned, &source);
    let consented = load_install_consent(env.user_dir.as_deref())?;
    let toolchain = load_toolchain_consent(env.user_dir.as_deref())?;
    // Whether the component is there is asked of `rustup` only once its
    // consent is recorded: before that, no `rustup` command runs at all.
    let rustup = installer.has_rustup();
    let missing: Vec<&Planned> = lines
        .iter()
        .filter(|p| !consented.covers(&p.tool, &p.version))
        .collect();
    let toolchain_missing = rustup && !toolchain.covers(LLVM_TOOLS);
    if (!missing.is_empty() || toolchain_missing) && !request.consent {
        let retry = match &request.from {
            Some(dir) => format!("{COVERAGE_INSTALL_ENTRY} --from {} --consent", shell_word(&dir.to_string_lossy())),
            None => format!("{COVERAGE_INSTALL_ENTRY} --consent"),
        };
        let named: Vec<String> = missing
            .iter()
            .map(|p| describe(p))
            .chain(toolchain_missing.then(toolchain_line))
            .collect();
        return Err(ConfigError::InvalidValue {
            key: "consent".to_string(),
            message: format!(
                "no install consent is recorded for {}. Nothing was downloaded and no rustup command \
                 ran. Run `{retry}` in a terminal to record it (an MCP client cannot); it covers these \
                 in every project",
                named.join(", ")
            ),
        }
        .into());
    }
    let pairs: Vec<(&str, &str)> = missing
        .iter()
        .map(|p| (p.tool.as_str(), p.version.as_str()))
        .collect();
    let mut consent_recorded: Vec<String> = pairs.iter().map(|(tool, version)| format!("{tool} {version}")).collect();
    if !pairs.is_empty() {
        record_install_consent(env.user_dir.as_deref(), &pairs, &[])?;
    }
    if toolchain_missing {
        record_toolchain_consent(env.user_dir.as_deref(), &[LLVM_TOOLS])?;
        consent_recorded.push(LLVM_TOOLS_ADD.to_string());
    }

    let tools = if planned.is_empty() {
        Vec::new()
    } else {
        installer.install_with(&planned, &source, false, &mut |step| progress(&CoverageInstallStep::Install(step)))?.tools
    };
    let llvm_tools = if crate::jobs::cancelled() {
        ComponentOutcome::Cancelled
    } else if rustup {
        progress(&CoverageInstallStep::Component(ComponentStep::ComponentStarted {
            component: LLVM_TOOLS,
            command: LLVM_TOOLS_ADD,
        }));
        installer.add_llvm_tools()
    } else {
        super::rustup::no_rustup()
    };
    progress(&CoverageInstallStep::Component(ComponentStep::ComponentFinished { component: LLVM_TOOLS, outcome: &llvm_tools }));
    Ok(CoverageInstall {
        consent_recorded,
        kept,
        tools,
        llvm_tools,
    })
}

/// How the consent names the `llvm-tools-preview` step.
fn toolchain_line() -> String {
    format!("`{LLVM_TOOLS_ADD}` on your active Rust toolchain, run only if the component is missing")
}

/// One line of the Coverage page's consent before **Install** (S-750): a
/// managed coverage tool to download, or the `llvm-tools-preview` step — each
/// as `logos coverage install` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "line", rename_all = "snake_case")]
pub enum CoverageInstallLine {
    /// A pinned tool to download: its version, source host and size.
    Tool {
        tool: String,
        version: String,
        source: String,
        size: u64,
        /// What the dialog shows: `cargo-llvm-cov 0.9.1 from github.com (1.5 MB)`.
        text: String,
    },
    /// `rustup component add llvm-tools-preview` on the user's toolchain.
    Toolchain { command: &'static str, text: String },
}

impl CoverageInstallLine {
    /// What the dialog's answer echoes to accept this line: `<tool>
    /// <version>`, or the `rustup` command.
    #[must_use]
    pub fn accept(&self) -> String {
        match self {
            CoverageInstallLine::Tool { tool, version, .. } => format!("{tool} {version}"),
            CoverageInstallLine::Toolchain { command, .. } => (*command).to_string(),
        }
    }
}

/// The lines of a coverage install the consent does not cover yet, exactly
/// those [`coverage_install`] refuses without them: each managed coverage
/// tool with no usable copy whose version the install consent does not name,
/// then — while a `rustup` is on the search path — the `llvm-tools-preview`
/// step until the toolchain consent names it. Decides on recorded state only
/// (S-766): no copy's version is probed, so a `PATH` copy counts as usable,
/// as the page shows it installed; the probed install can then only keep
/// more. Starts no process and writes nothing.
///
/// # Errors
/// As [`coverage_install`], the consent aside.
pub fn coverage_install_pending(
    root: &Path,
    env: &ScanEnvironment,
    manifest: &Manifest,
) -> Result<Vec<CoverageInstallLine>> {
    let runner = CoverageRunner::open_with(root, env.user_dir.as_deref(), env.search_path.clone())?
        .with_tools_dir(env.tools_dir.clone());
    let planned: Vec<String> = manifest
        .coverage
        .iter()
        .filter(|entry| kept_coverage_copy(&runner, manifest, &entry.tool, Probes::Never).is_none())
        .map(|entry| entry.tool.clone())
        .collect();
    let installer = if planned.is_empty() {
        toolchain_installer(root, env, manifest)
    } else {
        installer(root, env, manifest)?
    };
    let consented = load_install_consent(env.user_dir.as_deref())?;
    let toolchain = load_toolchain_consent(env.user_dir.as_deref())?;
    let mut lines: Vec<CoverageInstallLine> = plan(&installer, &planned, &Source::Network)
        .into_iter()
        .filter(|p| !consented.covers(&p.tool, &p.version))
        .map(|p| CoverageInstallLine::Tool { text: describe(&p), tool: p.tool, version: p.version, source: p.source, size: p.size })
        .collect();
    if installer.has_rustup() && !toolchain.covers(LLVM_TOOLS) {
        lines.push(CoverageInstallLine::Toolchain { command: LLVM_TOOLS_ADD, text: toolchain_line() });
    }
    Ok(lines)
}

/// Record the consent for `lines`, as `logos coverage install --consent`
/// records it: each tool's `<tool> <version>` in the install consent, the
/// `llvm-tools-preview` step in the toolchain consent. User-level, never in
/// the repository.
///
/// # Errors
/// An I/O or format error writing either record.
pub fn record_coverage_install_lines(user_dir: Option<&Path>, lines: &[CoverageInstallLine]) -> Result<()> {
    let pairs: Vec<(&str, &str)> = lines
        .iter()
        .filter_map(|line| match line {
            CoverageInstallLine::Tool { tool, version, .. } => Some((tool.as_str(), version.as_str())),
            CoverageInstallLine::Toolchain { .. } => None,
        })
        .collect();
    if !pairs.is_empty() {
        record_install_consent(user_dir, &pairs, &[])?;
    }
    if lines.iter().any(|line| matches!(line, CoverageInstallLine::Toolchain { .. })) {
        record_toolchain_consent(user_dir, &[LLVM_TOOLS])?;
    }
    Ok(())
}

/// The copy of coverage tool `tool` an install leaves as it is, by the
/// precedence detection uses: any override, a `PATH` copy at or above the
/// minimum — or, under [`Probes::Never`], any `PATH` copy, its version
/// unread — or a managed copy.
fn kept_coverage_copy(runner: &CoverageRunner, manifest: &Manifest, tool: &str, probes: Probes) -> Option<KeptCoverageTool> {
    let kept = |origin, binary: &Path, version: Option<String>| KeptCoverageTool {
        tool: tool.to_string(),
        origin,
        binary: binary.to_path_buf(),
        version,
    };
    match runner.copy_in_use(tool, probes)? {
        Detection::Installed { binary, version, origin } => {
            let usable = origin != Origin::Path
                || probes == Probes::Never
                || manifest.meets_minimum(tool, version.as_deref());
            usable.then(|| kept(origin, &binary, version))
        }
        // A set override is the user's choice even when it cannot run: a
        // managed copy installed beside it would never be used.
        _ => runner.override_of(tool).map(|path| kept(Origin::Override, path, None)),
    }
}

/// The installer for the `llvm-tools-preview` step alone: it looks `rustup`
/// up on the search path outside the project and writes nothing, so it needs
/// neither a tools directory nor a platform with managed installs.
fn toolchain_installer(root: &Path, env: &ScanEnvironment, manifest: &Manifest) -> Installer {
    Installer::new(env.tools_dir.clone().unwrap_or_default())
        .with_manifest(manifest.clone())
        .with_search_path(env.search_path.clone())
        .for_project(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::FailStage;

    #[test]
    fn the_consent_names_the_version_source_and_size_in_decimal_megabytes() {
        let line = |size| {
            describe(&Planned {
                tool: "trivy".to_string(),
                version: "0.74.0".to_string(),
                source: "github.com".to_string(),
                size,
                for_tools: Vec::new(),
            })
        };
        // Trivy 0.74.0's macOS arm64 archive: 47 694 894 bytes, 45.5 MiB.
        assert_eq!(line(47_694_894), "trivy 0.74.0 from github.com (47.7 MB)");
        assert_eq!(line(2_790_298), "trivy 0.74.0 from github.com (2.8 MB)");
    }

    #[test]
    fn an_install_exits_1_only_when_nothing_it_was_asked_for_is_in_place() {
        let tool = |outcome| ToolInstall { tool: "gosec".to_string(), version: "2.21.4".to_string(), outcome };
        let binary = || PathBuf::from("/tools/gosec");
        let failed = || tool(InstallOutcome::Failed { stage: FailStage::Download, reason: "offline".to_string() });
        let kept = KeptTool {
            tool: "trivy".to_string(),
            copy: InstalledCopy { copy: CopyKind::Path, binary: binary(), version: None, update_available: None, unprobed: None },
        };
        let code = |kept: Vec<KeptTool>, tools: Vec<ToolInstall>| {
            SecurityInstall { consent_recorded: Vec::new(), licence_recorded: Vec::new(), kept, tools }.exit_code()
        };
        // Every tool asked for failed, was unavailable or was cancelled: 1.
        assert_eq!(code(Vec::new(), vec![failed()]), 1);
        let unavailable = tool(InstallOutcome::Unavailable { reason: "no artifact" });
        assert_eq!(code(Vec::new(), vec![failed(), unavailable, tool(InstallOutcome::Cancelled)]), 1);
        // One in place is enough, installed now, already there or kept as the user's own.
        assert_eq!(code(Vec::new(), vec![failed(), tool(InstallOutcome::Installed { binary: binary() })]), 0);
        assert_eq!(code(Vec::new(), vec![tool(InstallOutcome::AlreadyPresent { binary: binary() }), failed()]), 0);
        assert_eq!(code(vec![kept], vec![failed()]), 0);
        // Nothing attempted and nothing kept asked for nothing: 0.
        assert_eq!(code(Vec::new(), Vec::new()), 0);
    }

    #[test]
    fn the_named_command_quotes_a_from_directory_that_needs_it() {
        let from = |dir: &str| {
            retry_command(
                &InstallRequest {
                    tools: Selection::All,
                    update: false,
                    from: Some(PathBuf::from(dir)),
                    consent: false,
                    accept_licences: Vec::new(),
                },
                &[],
            )
        };
        assert_eq!(
            from("/srv/tools-1.0"),
            "logos security install --all --from /srv/tools-1.0 --consent"
        );
        assert_eq!(
            from("/tmp/my artifacts"),
            "logos security install --all --from '/tmp/my artifacts' --consent"
        );
        assert_eq!(
            from("/tmp/it's"),
            "logos security install --all --from '/tmp/it'\\''s' --consent"
        );
    }
}
