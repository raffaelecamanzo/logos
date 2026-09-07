//! **S-365 — configuration-key resolvability and profile agreement**
//! ([CR-115] CRA-01, [CR-117] CRA-01, [FR-WS-08], [FR-WS-10], [FR-SY-11]).
//!
//! The S-355 measurement in the parent module ended where the values stop being
//! in the code: **81 of 98** Java client-call sites resolve to a *configuration
//! lookup* — a getter on a cross-unit `@ConfigurationProperties` bean whose
//! value lives in `application.yml`. [CR-115] proposes reading those sources.
//! [CR-117] §3.3 proposes the same mechanism for a broker publish site's topic.
//! Both are gated on the same question, and this module answers it:
//!
//! > Of the sites that resolve to a configuration key, how many resolve to a
//! > key on whose value **every committed source agrees**?
//!
//! [CR-115] §3.4 makes agreement the admission rule, not a tie-break: a key
//! several sources define differently is **refused**, naming the key and the
//! conflicting files, rather than defaulted to the unprofiled value. So the
//! newly-admitted count is bounded by agreement, and agreement is what is
//! measured here.
//!
//! # One gate, two arms
//!
//! The two corpora are measured by one run, reported **separately and
//! combined, never averaged** — a material figure for one arm and an immaterial
//! figure for the other is a real outcome, and [CR-115]/[CR-117] are then
//! decided differently from each other. The arms share this module's key
//! resolution and agreement rule verbatim, which is the point: two measurements
//! drifting to two answers about one `@ConfigurationProperties` mechanism is
//! exactly what [CR-117] §3.3 asks to be avoided.
//!
//! | arm | corpus (the denominator) | admitted today |
//! |-----|--------------------------|----------------|
//! | client call | a verb-anchored `invocations` match inside a ledger-gate-admitted file (S-355's corpus) whose least-resolvable operand is a configuration lookup | a single static literal |
//! | broker publish | a **message-header publish form** — `setHeader(KafkaHeaders.TOPIC, <operand>)` — in a file of a language shipping `brokers` | nothing: the real `brokers.scm` matches only a `(string_literal)` topic in argument position, and never the header form at all |
//!
//! The denominators differ and are printed separately for that reason. The
//! broker arm has **no ledger gate** (`brokers.scm` is not detector-gated) and
//! no already-admitted subset, so its ratio is not comparable to the client
//! arm's by construction.
//!
//! ## Why the broker arm is the header form only
//!
//! `brokers.scm`'s other publish pattern — `send`/`convertAndSend`/`publish`
//! with a literal first argument — was **deliberately not** widened to
//! non-literal first arguments for this measurement. Lifting the literal
//! constraint matches every `.send(x)` in the corpus, including
//! `kafkaTemplate.send(message)` one line below a header-form publish, and the
//! resulting denominator would be noise. [CR-117] §3.3 scopes the capture work
//! to the header form; the measurement scopes to the same thing, and says so
//! rather than reporting a bigger number over a corpus it cannot defend.
//!
//! # What "resolves to a configuration key" means
//!
//! Spring's binding is a two-hop lookup and each hop can fail, so each failure
//! is a named [`Refusal`] rather than a silent drop ([NFR-CC-04]):
//!
//! ```text
//! mailServerConfigurationApi.getUriGetArchive()
//!   │                        └─ getter → property `uriGetArchive`
//!   └─ field of declared type `MailServerConfigurationApi`
//!        └─ @ConfigurationProperties(prefix = "mailserver.api")
//!             → key `mailserver.api.uri-get-archive`
//! ```
//!
//! A `@Value("${key:default}")`-annotated name resolves in one hop. An
//! environment read (`System.getenv`, `process.env`, `os.Getenv`) resolves to
//! the variable's name — deliberately, so it can be looked up and reported as
//! *not defined by any committed source* rather than as an unrecognised shape.
//!
//! Keys are compared under Spring's **relaxed binding**: each `.`-segment is
//! lower-cased with `-` and `_` removed, so `uri-get-archive`, `uriGetArchive`
//! and `URI_GET_ARCHIVE` are one key.
//!
//! # Which sources count
//!
//! `application.yml`, `application.yaml`, `application.properties` and every
//! `application-<profile>` variant of those three extensions, discovered by the
//! same [FR-SY-11] admission walk the parent module's corpus scan uses
//! (`.gitignore` honoured, nested-git boundaries pruned, no machine-local
//! ignore files). Test resources are **not** excluded: they are committed
//! sources that define the key, and [CR-115] §3.4's rule says *every discovered
//! source*.
//!
//! ## Two scopes, both reported
//!
//! Agreement is computed twice and the difference is itself a finding:
//!
//! - **module scope** (the headline) — sources under the call site's nearest
//!   ancestor holding a build descriptor (`pom.xml`, `build.gradle{,.kts}`,
//!   `package.json`, `go.mod`). This approximates the classpath one deployable
//!   actually assembles, which is the scope [CR-115]'s base-URL rule is about.
//! - **workspace scope** — every source in the corpus. This is the scope
//!   [CR-117]'s *canonical topic identity* needs, because a publish in one
//!   member must meet a subscribe in another.
//!
//! # These tables stay here
//!
//! `HEADER_PUBLISH_QUERY`, `BASE_URL_METHODS`, [`names_topic_header`], the
//! `@ConfigurationProperties` index and the relaxed-binding rules are
//! Spring-API-coupled measurement tables. They fall under the parent harness's
//! carve-out (see its module docs) and **must not be lifted into
//! `logos-core/src/resolve/`**, which
//! `resolve::framework::tests::jvm_parity::no_language_specific_composition_code_exists`
//! forbids. The real arm's capture belongs in `plugins/<lang>/*.scm` as pure
//! plugin data ([ADR-54]); a measurement is allowed to know what a framework
//! looks like, a resolver is not.
//!
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
//!
//! # Materiality is declared before the run, not after it
//!
//! [CR-113] closed because folding admitted zero. "Immaterial" must not be a
//! number chosen once the number is known, so the floor is a constant here:
//! a mechanism that recovers fewer than [`MATERIAL_FLOOR_PCT`]% of the sites it
//! targets, or fewer than [`MATERIAL_FLOOR_SITES`] sites outright, repeats
//! [CR-113]'s outcome and the change request should close the same way.
//!
//! # Recorded finding (2026-09-07, `~/source/pec-services`, 84 members)
//!
//! **[CR-115] CRA-01 holds. [CR-117] CRA-01 is falsified on production source.**
//! The two arms answer differently, and the split that decides them is one
//! level deeper than the arms: **production source versus test source**. The
//! broker arm admits 38 of 54 overall — and every one of the 38 is an IT test
//! class, while all 13 of its production publish sites are refused.
//!
//! [`RECORDED_FINDING`] carries the full text and the tables; it is
//! `include_str!`-ed so the run prints exactly what the documents quote, rather
//! than a second hand-maintained copy that can drift from it. The verdict —
//! per arm, read off the production row — is asserted in
//! [`measure_configuration_agreement_over_the_reference_workspace`].
//!
//! [CR-113]: ../../docs/requests/CR-113-constant-folded-base-url-composition.md
//! [CR-115]: ../../docs/requests/CR-115-configuration-bound-base-url-resolution.md
//! [CR-117]: ../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
//! [FR-SY-11]: ../../docs/specs/requirements/FR-SY-11.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-10]: ../../docs/specs/requirements/FR-WS-10.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use tree_sitter::{Node, Parser};

use super::{folded_text, operand_name, static_literal, OperandKind, Unit, FOLD_DEPTH};

// ── Declared thresholds ─────────────────────────────────────────────────────

/// Percent of an arm's own denominator below which the mechanism is immaterial.
pub const MATERIAL_FLOOR_PCT: usize = 10;

/// Absolute site count below which the mechanism is immaterial whatever the
/// percentage says — 3 of 8 is a large share of nothing.
pub const MATERIAL_FLOOR_SITES: usize = 5;

/// The recorded verdict, reproduced by the run and pinned by its assertion.
pub const RECORDED_FINDING: &str = include_str!("configuration_agreement_finding.txt");

// ── Configuration sources ───────────────────────────────────────────────────

/// The three configuration file stems Spring loads, and the profile-variant
/// forms of each. Names, not a glob: `bootstrap.yml`, `application.json` and a
/// `config/` override directory are out of scope and their absence is stated
/// rather than assumed (the corpus contains none).
const CONFIG_EXTENSIONS: [&str; 3] = ["yml", "yaml", "properties"];

/// Build descriptors whose directory is a module root — the scope one
/// deployable's configuration is assembled from.
const MODULE_DESCRIPTORS: [&str; 5] =
    ["pom.xml", "build.gradle", "build.gradle.kts", "package.json", "go.mod"];

/// One discovered configuration source.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    /// Corpus-relative path.
    pub path: String,
    /// The `application-<profile>` profile, or `None` for the unprofiled file.
    pub profile: Option<String>,
    /// Module root this source belongs to (corpus-relative, `""` at the root).
    pub module: String,
    /// Canonical key → the values this file proves for it. A file that defines
    /// one key twice (multi-document YAML) proves both.
    pub values: BTreeMap<String, BTreeSet<String>>,
}

/// Every configuration source in the corpus, plus the module partition.
#[derive(Debug, Default)]
pub struct ConfigCorpus {
    pub sources: Vec<ConfigSource>,
    /// Module roots, longest-prefix-matched to place a file.
    modules: BTreeSet<String>,
    /// Java files mentioning `ConfigurationProperties`, stashed by the same
    /// walk so the class index needs no second traversal of the corpus.
    props_candidates: Vec<String>,
}

/// What a key's committed sources prove about its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agreement {
    /// Exactly one value across every source that defines the key.
    Agreed { value: String, sources: usize },
    /// Two or more distinct values — refused by [CR-115] §3.4, with the
    /// conflicting files named.
    Disagreed { values: Vec<(String, Vec<String>)> },
    /// Defined, but at least one source's value is itself a `${…}` indirection,
    /// so the sources do not prove a value at all.
    Placeholder { sources: usize },
    /// No committed source defines the key.
    Missing,
}

impl Agreement {
    /// The full statement [CR-115] AC2 asks for: how many committed sources
    /// define the key **and** whether they agree. `label()` alone answered only
    /// the second half, so an agreed census line never said how much evidence
    /// stood behind it.
    pub fn detail(&self) -> String {
        match self {
            Self::Agreed { value, sources: 0 } => {
                format!("proven at the call site: {value:?}")
            }
            Self::Agreed { value, sources } => {
                format!("agreed across {sources} source(s): {value:?}")
            }
            Self::Disagreed { values } => format!(
                "disagreement across {} source(s), {} distinct values",
                values.iter().map(|(_, files)| files.len()).sum::<usize>(),
                values.len(),
            ),
            Self::Placeholder { sources } => {
                format!("defined by {sources} source(s), but the value is a `${{…}}` indirection")
            }
            Self::Missing => "no committed source defines it".to_string(),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Agreed { .. } => "agreed",
            Self::Disagreed { .. } => "disagreement",
            Self::Placeholder { .. } => "placeholder value",
            Self::Missing => "missing key",
        }
    }

    fn value(&self) -> Option<&str> {
        match self {
            Self::Agreed { value, .. } => Some(value),
            _ => None,
        }
    }
}

impl ConfigCorpus {
    /// Walk the corpus once: discover configuration sources, module roots, and
    /// the Java files that may declare a `@ConfigurationProperties` class.
    ///
    /// The walker is configured exactly as the parent module's `measure` walk —
    /// `parents(false)`, `git_global(false)`, `ignore(false)` — so the two
    /// passes admit the same population and a machine-local `~/.gitignore` can
    /// never change a published figure.
    pub fn discover(root: &Path) -> Self {
        let mut corpus = Self::default();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .ignore(false)
            .parents(false)
            .build();
        let mut raw: Vec<(String, Option<String>, String)> = Vec::new();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();

            if MODULE_DESCRIPTORS.contains(&name.as_str()) {
                corpus.modules.insert(parent_dir(&rel).to_string());
                continue;
            }
            if name.ends_with(".java") {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    if text.contains("ConfigurationProperties") {
                        corpus.props_candidates.push(rel.clone());
                    }
                }
                continue;
            }
            let Some(profile) = config_profile(&name) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            raw.push((rel, profile, text));
        }
        // Module placement needs the full module set, so sources are built in a
        // second pass over what the walk collected — not a second walk.
        for (path, profile, text) in raw {
            let values = if path.ends_with(".properties") {
                parse_properties(&text)
            } else {
                parse_yaml(&text)
            };
            let module = corpus.module_of(&path).to_string();
            corpus.sources.push(ConfigSource { path, profile, module, values });
        }
        corpus.sources.sort_by(|a, b| a.path.cmp(&b.path));
        corpus
    }

    /// The module root owning a corpus-relative path: the longest module
    /// directory that is a path-segment prefix of it, or `""` at the root.
    pub fn module_of(&self, rel: &str) -> &str {
        self.modules
            .iter()
            .filter(|m| m.is_empty() || rel.starts_with(&format!("{m}/")))
            .max_by_key(|m| m.len())
            .map_or("", String::as_str)
    }

    /// What the sources in `scope` prove about `key`. `scope` is a module root,
    /// or `None` for the whole workspace.
    pub fn agreement(&self, key: &str, scope: Option<&str>) -> Agreement {
        let canonical = canonical_key(key);
        let mut by_value: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut sources = 0usize;
        let mut placeholder = false;
        for source in &self.sources {
            if scope.is_some_and(|s| source.module != s) {
                continue;
            }
            let Some(values) = source.values.get(&canonical) else {
                continue;
            };
            sources += 1;
            for value in values {
                if value.contains("${") {
                    placeholder = true;
                }
                by_value.entry(value.clone()).or_default().push(source.path.clone());
            }
        }
        if sources == 0 {
            return Agreement::Missing;
        }
        if placeholder {
            return Agreement::Placeholder { sources };
        }
        if by_value.len() == 1 {
            let value = by_value.keys().next().cloned().unwrap_or_default();
            return Agreement::Agreed { value, sources };
        }
        Agreement::Disagreed { values: by_value.into_iter().collect() }
    }

    /// Profiles discovered across the corpus, for the census line.
    pub fn profiles(&self) -> BTreeSet<&str> {
        self.sources.iter().filter_map(|s| s.profile.as_deref()).collect()
    }
}

/// The directory part of a corpus-relative path (`""` for a root-level file).
fn parent_dir(rel: &str) -> &str {
    rel.rfind('/').map_or("", |i| &rel[..i])
}

/// The Spring profile of a configuration filename, or `None` when the file is
/// not a configuration source at all.
///
/// `Some(None)` is the unprofiled `application.<ext>`; `Some(Some(p))` is
/// `application-<p>.<ext>`. A hyphen inside the profile is kept whole, so
/// `application-it-jenkins.yml` is the profile `it-jenkins`, not `it`.
#[allow(clippy::option_option)]
fn config_profile(name: &str) -> Option<Option<String>> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !CONFIG_EXTENSIONS.contains(&ext) {
        return None;
    }
    if stem == "application" {
        return Some(None);
    }
    let profile = stem.strip_prefix("application-")?;
    (!profile.is_empty()).then(|| Some(profile.to_string()))
}

/// Spring's relaxed binding, applied per `.`-segment: lower-case, and drop `-`
/// and `_`. `mailserver.api.uri-get-archive` and `mailserver.api.uriGetArchive`
/// canonicalise to the same key.
pub fn canonical_key(key: &str) -> String {
    key.split('.')
        .map(|segment| {
            segment
                .chars()
                .filter(|c| *c != '-' && *c != '_')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Strip a YAML inline comment, honouring quotes.
///
/// Separate from [`canonical_value`] and applied **before** the empty/block
/// tests in [`parse_yaml`], because a comment changes what those tests see:
/// `api:   # the mail API` is a mapping header, not a scalar, and reading it as
/// one re-parents its whole subtree.
///
/// A quoted scalar ends at its closing quote and anything after it is comment;
/// an unquoted one ends at the first ` #`. A bare `#` at the start is a
/// comment entire.
fn strip_yaml_comment(rest: &str) -> &str {
    let rest = rest.trim();
    for quote in ['"', '\''] {
        if let Some(body) = rest.strip_prefix(quote) {
            return match body.find(quote) {
                // Keep both quotes; `canonical_value` unwraps them.
                Some(end) => &rest[..end + 2],
                None => rest,
            };
        }
    }
    if rest.starts_with('#') {
        return "";
    }
    match rest.find(" #") {
        Some(hash) => rest[..hash].trim_end(),
        None => rest,
    }
}

/// A configuration value as the sources prove it: trimmed, with one layer of
/// matching quotes removed.
///
/// Comment stripping is **not** done here. It is a YAML rule and this runs for
/// `.properties` too, where `#` is only a comment at the start of a line and a
/// quote is a literal character — putting `.properties` values through the YAML
/// rules turned `app.url=/x # main` into `/x`, which is a different value from
/// the one Java's `Properties` reads and could make two sources falsely agree.
fn canonical_value(raw: &str) -> String {
    let text = raw.trim();
    let quoted = (text.starts_with('"') && text.ends_with('"') && text.len() >= 2)
        || (text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2);
    if quoted {
        return text[1..text.len() - 1].to_string();
    }
    text.to_string()
}

// ── Configuration source parsing ────────────────────────────────────────────

/// Flatten the scalar subset of YAML the corpus's `application*.yml` files use:
/// nested mappings of scalars, `---` document separators, `#` comments.
///
/// A **deliberate subset**, not a YAML parser. Block scalars (`|`, `>`, and
/// their `|+`/`>-`/`|2` indicator forms) and sequences bind no scalar key here,
/// so a key whose value is a list is simply absent — which the agreement rule
/// then reports as `missing key` rather than as an agreed value it never read.
/// Under-reading is the safe direction: it can only lower the newly-admitted
/// count, never inflate it.
///
/// Both skips must consume the **body**, not just the header line. Skipping a
/// `-` item line while leaving the indent stack untouched let the item's own
/// continuation lines register as direct children of the enclosing mapping, so
/// a `routes:` list fabricated a real-looking `spring.cloud.gateway.routes.uri`
/// key from a list element — an invented value, not an absent one.
pub fn parse_yaml(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // (indent, key) of every open mapping level.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut skip_block_until: Option<usize> = None;
    let mut skip_sequence_until: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if let Some(block_indent) = skip_block_until {
            // A blank line inside a block scalar is part of it. Ending the skip
            // there would let the block's remaining lines register keys.
            if trimmed.is_empty() || indent > block_indent {
                continue;
            }
            skip_block_until = None;
        }
        if let Some(seq_indent) = skip_sequence_until {
            // The continuation lines of a `- name: x` item are indented past
            // the dash and are the ITEM's keys, not the parent mapping's.
            if trimmed.is_empty() || indent > seq_indent {
                continue;
            }
            skip_sequence_until = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed == "---" || trimmed.starts_with("--- ") {
            stack.clear();
            continue;
        }
        if trimmed.starts_with('-') {
            skip_sequence_until = Some(indent);
            continue;
        }
        let Some((key, rest)) = split_yaml_entry(trimmed) else {
            continue;
        };
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        // Before anything is decided about `rest`: a trailing comment must not
        // make a mapping header look like a scalar.
        let rest = strip_yaml_comment(rest);
        if rest.is_empty() {
            stack.push((indent, key));
            continue;
        }
        // Any block-scalar indicator, not just the four bare spellings: `|+`,
        // `>-`, `|2` and friends all introduce a body that is not YAML. A plain
        // scalar never begins with `|` or `>`, and a quoted one took the branch
        // above.
        if rest.starts_with('|') || rest.starts_with('>') {
            skip_block_until = Some(indent);
            continue;
        }
        // An anchor or alias (`&name`, `*name`) is not a value: `defaults: &d`
        // is a mapping header whose children must stay under it.
        if rest.starts_with('&') {
            stack.push((indent, key));
            continue;
        }
        // A flow collection (`[a, b]`, `{k: v}`) is not a scalar. Recording its
        // source text as the value would let a list-valued key resolve to the
        // string "[a, b]" and be reported as agreed — a mis-read, not an
        // under-read, and the one direction this parser must not take.
        if rest.starts_with('[') || rest.starts_with('{') {
            continue;
        }
        let path: Vec<&str> =
            stack.iter().map(|(_, k)| k.as_str()).chain(std::iter::once(key.as_str())).collect();
        out.entry(canonical_key(&path.join(".")))
            .or_default()
            .insert(canonical_value(rest));
    }
    out
}

/// Split a YAML mapping line into its key and the rest, honouring a quoted key
/// and refusing a line with no `:` at all.
fn split_yaml_entry(line: &str) -> Option<(String, &str)> {
    for quote in ['"', '\''] {
        if let Some(rest) = line.strip_prefix(quote) {
            let end = rest.find(quote)?;
            let after = rest[end + 1..].trim_start();
            return Some((rest[..end].to_string(), after.strip_prefix(':')?));
        }
    }
    let colon = line.find(':')?;
    let key = line[..colon].trim();
    // A YAML key is one token. Anything with whitespace in it is the tail of a
    // construct this subset does not read — a flow mapping, or a continuation
    // line of a value that happens to contain a colon — and must not register
    // a key.
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    Some((key.to_string(), &line[colon + 1..]))
}

/// Parse a `.properties` file: `key=value` or `key:value`, `#`/`!` comments,
/// and trailing-backslash line continuation.
///
/// Two rules that differ from YAML and cost a wrong value if borrowed from it:
/// a line continues only on an **odd** number of trailing backslashes (`a=C:\\`
/// is a value ending in one literal backslash, not a continuation that swallows
/// the next entry), and `#` is a comment only at the start of a line, never
/// inline. Values are therefore trimmed but not comment-stripped or unquoted.
pub fn parse_properties(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pending = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if pending.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!')) {
            continue;
        }
        let trailing_backslashes = trimmed.chars().rev().take_while(|c| *c == '\\').count();
        if trailing_backslashes % 2 == 1 {
            let head = &trimmed[..trimmed.len() - 1];
            pending.push_str(head.trim_end());
            continue;
        }
        let joined = format!("{pending}{trimmed}");
        pending.clear();
        let Some((key, value)) = joined
            .find(['=', ':'])
            .map(|i| (joined[..i].trim(), &joined[i + 1..]))
        else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        // Trim only: see the note above on why the YAML value rules must not
        // reach a `.properties` value.
        out.entry(canonical_key(key)).or_default().insert(value.trim().to_string());
    }
    out
}

// ── `@ConfigurationProperties` class index ──────────────────────────────────

/// One `@ConfigurationProperties` class: its prefix and the property names it
/// declares (fields, or record components under constructor binding).
#[derive(Debug, Clone)]
pub struct PropertiesClass {
    pub prefix: String,
    /// Canonicalised property names, so a getter matches without re-deriving
    /// the source spelling.
    pub properties: BTreeSet<String>,
    pub file: String,
    /// The module root declaring it — the scope a use site prefers.
    pub module: String,
}

/// Every `@ConfigurationProperties` class in the corpus, by **simple** type
/// name — each name keeping *every* declaration of it.
///
/// Simple names, not fully-qualified ones: a field's declared type is written
/// unqualified at the use site, and resolving imports would be a second
/// resolver. The corpus makes that cheap reading unsafe on its own: 84 members
/// declare `MailServerConfigurationApi` more than once, under one prefix but
/// with **different property sets**, so a workspace-wide "first wins" index
/// silently resolved `archive-manager`'s getter against `archive-api`'s class
/// and reported eleven false `property not declared` refusals. So the lookup is
/// **module-scoped first**: a use site prefers the class its own module
/// declares, and only falls back to the workspace when the remaining
/// declarations agree with each other. When they do not, the name is a
/// [`PropertiesIndex::collisions`] entry and resolves to nothing rather than to
/// a guess.
#[derive(Debug, Default)]
pub struct PropertiesIndex {
    classes: BTreeMap<String, Vec<PropertiesClass>>,
    pub collisions: BTreeSet<String>,
    /// Classes annotated but carrying no readable prefix (`@ConfigurationProperties`
    /// on a `@Bean` method, or a prefix that is not a string literal).
    pub prefixless: usize,
}

impl PropertiesIndex {
    /// Parse the Java files the corpus walk stashed and index their annotated
    /// classes. Drives the real Java grammar, as every other pass here does.
    pub fn build(root: &Path, corpus: &ConfigCorpus, language: &tree_sitter::Language) -> Self {
        let mut index = Self::default();
        let mut parser = Parser::new();
        if parser.set_language(language).is_err() {
            return index;
        }
        for rel in &corpus.props_candidates {
            let Ok(source) = std::fs::read_to_string(root.join(rel)) else {
                continue;
            };
            let Some(tree) = parser.parse(&source, None) else {
                continue;
            };
            let module = corpus.module_of(rel).to_string();
            index.absorb(rel, &module, tree.root_node(), source.as_bytes());
        }
        index.seal();
        index
    }

    fn absorb(&mut self, rel: &str, module: &str, root: Node<'_>, src: &[u8]) {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
            drop(cursor);
            if !matches!(node.kind(), "class_declaration" | "record_declaration") {
                continue;
            }
            let Some(modifiers) = child_of_kind(node, "modifiers") else {
                continue;
            };
            let Some(annotation) = annotation_named(modifiers, "ConfigurationProperties", src)
            else {
                continue;
            };
            let Some(prefix) = annotation_prefix(annotation, src) else {
                self.prefixless += 1;
                continue;
            };
            let Some(name) = node.child_by_field_name("name").and_then(|n| n.utf8_text(src).ok())
            else {
                continue;
            };
            let class = PropertiesClass {
                prefix,
                properties: declared_properties(node, src),
                file: rel.to_string(),
                module: module.to_string(),
            };
            self.classes.entry(name.to_string()).or_default().push(class);
        }
    }

    /// Seal the index: a simple name whose declarations disagree — outside the
    /// module that will ask for it — is recorded as a collision.
    pub fn seal(&mut self) {
        for (name, declarations) in &self.classes {
            let distinct: BTreeSet<(&String, &BTreeSet<String>)> =
                declarations.iter().map(|c| (&c.prefix, &c.properties)).collect();
            if distinct.len() > 1 {
                self.collisions.insert(name.clone());
            }
        }
    }

    /// The class a use site in `module` sees: its own module's declaration
    /// first, then the workspace's when every remaining declaration agrees.
    pub fn get(&self, simple_type: &str, module: &str) -> Option<&PropertiesClass> {
        let declarations = self.classes.get(simple_type)?;
        // The own-module subset gets the same distinct-declaration test as the
        // workspace one: two classes of the same simple name in different
        // packages of ONE module is the same ambiguity the collision rule
        // exists for, and picking the first is the guess it forbids.
        let mut own = declarations.iter().filter(|c| c.module == module);
        if let Some(first) = own.next() {
            let ambiguous = own.any(|c| c.prefix != first.prefix || c.properties != first.properties);
            return (!ambiguous).then_some(first);
        }
        (!self.collisions.contains(simple_type)).then(|| declarations.first()).flatten()
    }

    /// Distinct class names indexed.
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// The annotation node of the given simple name inside a `modifiers` node.
fn annotation_named<'t>(modifiers: Node<'t>, name: &str, src: &[u8]) -> Option<Node<'t>> {
    let mut cursor = modifiers.walk();
    let found = modifiers.named_children(&mut cursor).find(|c| {
        matches!(c.kind(), "annotation" | "marker_annotation")
            && c.child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .is_some_and(|n| n == name)
    });
    found
}

/// The `prefix` of a `@ConfigurationProperties` annotation, in either of the
/// two forms the corpus uses: `(prefix = "x.y")` and the value form `("x.y")`.
fn annotation_prefix(annotation: Node<'_>, src: &[u8]) -> Option<String> {
    let args = annotation.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        match arg.kind() {
            "element_value_pair" => {
                let key = arg.child_by_field_name("key")?.utf8_text(src).ok()?;
                if key == "prefix" || key == "value" {
                    let value = arg.child_by_field_name("value")?;
                    return static_literal(value, src);
                }
            }
            "string_literal" => return static_literal(arg, src),
            _ => {}
        }
    }
    None
}

/// The canonicalised property names a properties class declares: its fields,
/// plus a record's components (Spring 3 constructor binding).
fn declared_properties(class: Node<'_>, src: &[u8]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![class];
    while let Some(node) = stack.pop() {
        // Do not descend into a nested type: its fields are that type's
        // properties, reached through a nested accessor this measurement
        // refuses rather than guesses at.
        if node.id() != class.id()
            && matches!(node.kind(), "class_declaration" | "record_declaration")
        {
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
        drop(cursor);
        match node.kind() {
            "field_declaration" => {
                let mut kids = node.walk();
                for declarator in node.named_children(&mut kids) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    if let Some(name) =
                        declarator.child_by_field_name("name").and_then(|n| n.utf8_text(src).ok())
                    {
                        out.insert(canonical_key(name));
                    }
                }
            }
            // A record's own components (Spring 3 constructor binding) — NOT
            // every parameter in the class body. Unrestricted, this registered
            // a setter's or helper's parameter as a declared property, which
            // loosens `PropertyNotDeclared` and, where a source happens to
            // define `<prefix>.<paramName>`, admits a site outright.
            "formal_parameter"
                if class.kind() == "record_declaration"
                    && node
                        .parent()
                        .is_some_and(|p| p.parent().is_some_and(|g| g.id() == class.id())) =>
            {
                if let Some(name) =
                    node.child_by_field_name("name").and_then(|n| n.utf8_text(src).ok())
                {
                    out.insert(canonical_key(name));
                }
            }
            _ => {}
        }
    }
    out
}

// ── Key resolution ──────────────────────────────────────────────────────────

/// Everything key resolution needs besides the expression itself: the
/// configuration sources, the properties classes, and the module scope both are
/// read in. A struct because passing the three separately pushed `judge` and
/// both collectors past the argument limit.
#[derive(Clone, Copy)]
pub struct Resolver<'a> {
    pub corpus: &'a ConfigCorpus,
    pub props: &'a PropertiesIndex,
    /// The module root the use site sits in; `""` is the corpus root.
    pub module: &'a str,
}

impl Resolver<'_> {
    /// What the sources in this scope prove about `key`.
    fn agreement(&self, key: &str) -> Agreement {
        self.corpus.agreement(key, Some(self.module))
    }
}

/// Why an accessor did **not** resolve to a configuration key. Each variant is
/// a distinct, countable fault so the residue is a diagnosis rather than a
/// bucket labelled "other".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// A getter chained on another call — `a.getB().getC()`. Resolving it needs
    /// the nested type's own binding, which this measurement does not guess.
    NestedAccessor,
    /// The operand is a method parameter: its value originates one call frame
    /// away, which is a distinct capability neither change request builds
    /// ([CR-117] CRA-04 counts exactly these).
    MethodParameter,
    /// A name the compilation unit does not bind at all.
    UnboundName,
    /// A name the unit binds to two different configuration accessors. The
    /// source does not prove which one the call site sees, so neither is used.
    AmbiguousBinding,
    /// The receiver resolves, but the member read is not a getter.
    NotAGetter,
    /// The receiver's declared type is not visible in the compilation unit.
    ReceiverTypeUnknown,
    /// The declared type declares no `@ConfigurationProperties` class in the
    /// corpus — the bean is external, or bound some other way.
    NoPropertiesClass,
    /// The class is indexed, but declares no property matching the getter.
    PropertyNotDeclared,
    /// A configuration-shaped operand in none of the recognised forms.
    UnrecognisedAccessor,
}

impl Refusal {
    pub fn label(self) -> &'static str {
        match self {
            Self::NestedAccessor => "nested accessor",
            Self::MethodParameter => "method parameter (one call frame away)",
            Self::UnboundName => "name unbound in this unit",
            Self::AmbiguousBinding => "name bound to two different accessors",
            Self::NotAGetter => "not a getter",
            Self::ReceiverTypeUnknown => "receiver type unknown",
            Self::NoPropertiesClass => "no @ConfigurationProperties class",
            Self::PropertyNotDeclared => "property not declared on the class",
            Self::UnrecognisedAccessor => "unrecognised accessor shape",
        }
    }

    pub const ALL: [Self; 9] = [
        Self::NestedAccessor,
        Self::MethodParameter,
        Self::UnboundName,
        Self::AmbiguousBinding,
        Self::NotAGetter,
        Self::ReceiverTypeUnknown,
        Self::NoPropertiesClass,
        Self::PropertyNotDeclared,
        Self::UnrecognisedAccessor,
    ];
}

/// How an operand reached its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// A getter on a `@ConfigurationProperties` bean.
    Properties,
    /// A `@Value("${key}")`-annotated name.
    ValueAnnotation,
    /// Not configuration at all: a literal or same-unit constant the call site
    /// itself proves. Carries the empty key.
    CallSite,
    /// An environment read. Resolved so the census can name the variable, but
    /// never admitted — [CR-115] §3.3 puts it out of scope.
    Environment,
}

impl KeySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Properties => "@ConfigurationProperties",
            Self::ValueAnnotation => "@Value",
            Self::CallSite => "the call site",
            Self::Environment => "the environment",
        }
    }
}

/// What one configuration-lookup operand resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Resolved {
        key: String,
        source: KeySource,
        /// The file declaring the `@ConfigurationProperties` class the key came
        /// from, so a census line names the evidence rather than asserting it
        /// ([NFR-CC-04]). `None` for a `@Value` or environment read, which
        /// carry their key at the use site.
        declared_in: Option<String>,
    },
    Unresolved(Refusal),
}

impl KeyOutcome {
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Resolved { key, .. } => Some(key),
            Self::Unresolved(_) => None,
        }
    }
}

/// Resolve one configuration-lookup operand to the configuration key it reads.
///
/// Recursion through same-unit bindings is bounded by [`FOLD_DEPTH`], the same
/// constant — and for the same reason — as `classify` and `folded_text` in the
/// parent module: it is the cycle guard. `a = b; b = a` binds two distinct AST
/// nodes, so an identity check alone does not terminate.
pub fn resolve_key(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    resolver: Resolver<'_>,
) -> KeyOutcome {
    resolve_key_at(node, src, unit, resolver, FOLD_DEPTH)
}

fn resolve_key_at(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    resolver: Resolver<'_>,
    depth: usize,
) -> KeyOutcome {
    if let Some(key) = environment_key(node, src) {
        return KeyOutcome::Resolved { key, source: KeySource::Environment, declared_in: None };
    }
    let kind = node.kind();
    if kind.contains("call") || kind.contains("invocation") {
        return resolve_getter(node, src, unit, resolver);
    }
    // A bare (or qualified) name: `@Value("${…}")` is the only one-hop form.
    if let Some(name) = operand_name(node, src) {
        // A parameter is decided FIRST, before any annotation is read — the
        // same rule `classify_binding` applies in the parent module.
        //
        // `Unit` is file-scoped, not scope-aware, so a `@Value`-annotated field
        // and a method parameter that shadows it share one entry, and the
        // field's key answered for the parameter. Testing "are ALL bindings
        // parameters?" does not settle it either, because the field binding
        // makes that false. So resolve it by SCOPE: does the method actually
        // enclosing this operand declare a parameter of that name? Field /
        // parameter collision is idiomatic Spring, and both readings of it are
        // wrong in a different direction.
        if encloses_parameter_named(node, &name, src) {
            return KeyOutcome::Unresolved(Refusal::MethodParameter);
        }
        let all_parameters = unit.bindings.get(&name).is_some_and(|bs| {
            bs.iter().all(|b| {
                super::is_parameter_kind(&b.bind_kind) || super::is_parameter_kind(&b.decl_kind)
            })
        });
        if all_parameters {
            return KeyOutcome::Unresolved(Refusal::MethodParameter);
        }
        if let Some(key) = value_annotation_key(&name, unit) {
            return KeyOutcome::Resolved {
                key,
                source: KeySource::ValueAnnotation,
                declared_in: None,
            };
        }
        // A name bound to a configuration accessor one hop away resolves
        // through that accessor — the same reach `classify` uses when it calls
        // an operand a configuration lookup in the first place.
        let Some(bindings) = unit.bindings.get(&name) else {
            return KeyOutcome::Unresolved(Refusal::UnboundName);
        };
        // Collect EVERY binding's key, not the first that resolves. `folded_text`
        // in the parent refuses a name bound to two different literals because
        // "the source does not prove which the call site sees"; the same is true
        // of a name bound to two different accessors, and `Unit::build` fills
        // this vec in DFS order, so "first" is not even source order.
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut resolved: Option<KeyOutcome> = None;
        for binding in bindings {
            let Some(value) = binding.value else { continue };
            if value.id() == node.id() || depth == 0 {
                continue;
            }
            let outcome = resolve_key_at(value, src, unit, resolver, depth - 1);
            if let Some(key) = outcome.key() {
                keys.insert(key.to_string());
                resolved.get_or_insert(outcome);
            }
        }
        match keys.len() {
            1 => return resolved.unwrap_or(KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor)),
            n if n > 1 => return KeyOutcome::Unresolved(Refusal::AmbiguousBinding),
            _ => {}
        }
    }
    KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor)
}

/// Whether a method/constructor/lambda enclosing `node` declares a parameter
/// called `name` — i.e. whether the operand refers to a parameter rather than
/// to a same-named field of the class.
fn encloses_parameter_named(node: Node<'_>, name: &str, src: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(scope) = current {
        if matches!(
            scope.kind(),
            "method_declaration" | "constructor_declaration" | "lambda_expression"
        ) {
            if let Some(params) = scope.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                let declared = params.named_children(&mut cursor).any(|p| {
                    p.child_by_field_name("name")
                        .and_then(|n| n.utf8_text(src).ok())
                        .is_some_and(|n| n == name)
                });
                if declared {
                    return true;
                }
            }
        }
        current = scope.parent();
    }
    false
}

/// `System.getenv("X")`, `os.Getenv("X")`, `process.env.X`, `os.environ["X"]`.
fn environment_key(node: Node<'_>, src: &[u8]) -> Option<String> {
    let text = node.utf8_text(src).ok()?.trim();
    if let Some(rest) = text.strip_prefix("process.env.") {
        return (!rest.is_empty() && rest.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .then(|| rest.to_string());
    }
    // Case-SENSITIVE needles. Lower-casing the operand first made
    // `configApi.getEnv("A")` — an ordinary bean getter — read as an
    // environment read and pre-empt the properties lookup. `getenv` and
    // `Getenv` are the actual spellings Java/PHP and Go use; `getEnv` is not
    // one of them.
    let (arg_start, close) = ["getenv(", "Getenv(", "environ[", "environ.get("]
        .iter()
        .find_map(|needle| {
            let at = text.find(needle)?;
            Some((at + needle.len(), if needle.contains('[') { ']' } else { ')' }))
        })?;
    let arg = text.get(arg_start..)?;
    let end = arg.find(close)?;
    // The operand must BE the environment read, not merely contain one.
    // `Optional.ofNullable(System.getenv("HOST")).orElse("/x")` is an
    // expression whose value the environment does not decide; reading it as
    // `HOST` silently dropped the fallback.
    if arg_start + end + 1 != text.len() {
        return None;
    }
    let name = arg[..end].trim().trim_matches(['"', '\'']);
    (!name.is_empty() && !name.contains(['(', ' ', '+'])).then(|| name.to_string())
}

/// The `${key}` of a `@Value` annotation on a same-unit binding of `name`.
fn value_annotation_key(name: &str, unit: &Unit<'_>) -> Option<String> {
    let bindings = unit.bindings.get(name)?;
    bindings
        .iter()
        .filter(|b| {
            // A parameter's `decl_head` is its own declaration, but a caller
            // supplies its value; an annotation elsewhere on a same-named field
            // does not describe it.
            !super::is_parameter_kind(&b.bind_kind) && !super::is_parameter_kind(&b.decl_kind)
        })
        .find_map(|b| {
        let at = b.decl_head.find("@Value")?;
        let head = &b.decl_head[at..];
        let open = head.find("${")?;
        let close = head[open..].find('}')? + open;
        let inner = &head[open + 2..close];
        // `${key:default}` — the default is not a source, so only the key.
        let key = inner.split(':').next().unwrap_or(inner).trim();
        (!key.is_empty()).then(|| key.to_string())
    })
}

/// `receiver.getProperty()` → the key its `@ConfigurationProperties` class binds.
fn resolve_getter(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    resolver: Resolver<'_>,
) -> KeyOutcome {
    let Some(function) = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("function"))
    else {
        return KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor);
    };
    let Some(receiver) = node.child_by_field_name("object") else {
        return KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor);
    };
    if receiver.kind().contains("call") || receiver.kind().contains("invocation") {
        return KeyOutcome::Unresolved(Refusal::NestedAccessor);
    }
    let method = function.utf8_text(src).unwrap_or_default().trim();
    let Some(property) = method
        .strip_prefix("get")
        .or_else(|| method.strip_prefix("is"))
        .filter(|p| !p.is_empty())
    else {
        return KeyOutcome::Unresolved(Refusal::NotAGetter);
    };
    let Some(receiver_name) = operand_name(receiver, src) else {
        return KeyOutcome::Unresolved(Refusal::ReceiverTypeUnknown);
    };
    let Some(declared) = unit.declared_type(&receiver_name) else {
        return KeyOutcome::Unresolved(Refusal::ReceiverTypeUnknown);
    };
    let Some(class) = resolver.props.get(declared, resolver.module) else {
        return KeyOutcome::Unresolved(Refusal::NoPropertiesClass);
    };
    let canonical = canonical_key(property);
    if !class.properties.contains(&canonical) {
        return KeyOutcome::Unresolved(Refusal::PropertyNotDeclared);
    }
    // Spring's property is the getter's suffix with its leading capital
    // lowered — `getUriGetArchive` binds `uriGetArchive`, which relaxed binding
    // then matches against `uri-get-archive`. The key is printed in that
    // spelling so a census line names something a reader can find in the yml.
    let mut chars = property.chars();
    let spelled = chars
        .next()
        .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
        .unwrap_or_default();
    KeyOutcome::Resolved {
        key: format!("{}.{spelled}", class.prefix),
        source: KeySource::Properties,
        declared_in: Some(class.file.clone()),
    }
}

// ── Per-site verdict ────────────────────────────────────────────────────────

/// What [CR-115] §3.4's agreement rule does with one site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every operand folds from the source alone, so configuration has no part
    /// in the site — S-355's territory, outside this measurement's reach.
    NotConfigurationBound,
    /// Already admitted today: the arm emits it without this change.
    AlreadyAdmitted,
    /// Newly admitted under the agreement rule, resolving to this value.
    NewlyAdmitted { resolved: String },
    /// Every key resolved and agreed, but the composition is still not a route
    /// the arm can bind (client-call arm only).
    NotARoute { resolved: String },
    /// At least one key's sources disagree.
    Disagreement,
    /// At least one key is defined by no committed source.
    MissingKey,
    /// At least one key's value is itself a `${…}` indirection.
    PlaceholderValue,
    /// At least one accessor does not resolve to a key.
    NoKey(Refusal),
    /// The value comes from an environment variable, which is not a committed
    /// source — out of scope for [CR-115] §3.3 whatever the sources say.
    OutOfScopeSource,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotConfigurationBound => "not configuration-bound",
            Self::AlreadyAdmitted => "already admitted",
            Self::NewlyAdmitted { .. } => "NEWLY ADMITTED",
            Self::NotARoute { .. } => "agreed, but not a route",
            Self::Disagreement => "refused: disagreement",
            Self::MissingKey => "refused: missing key",
            Self::PlaceholderValue => "refused: placeholder value",
            Self::NoKey(_) => "refused: no key",
            Self::OutOfScopeSource => "refused: environment variable, not a committed source",
        }
    }

    pub fn is_newly_admitted(&self) -> bool {
        matches!(self, Self::NewlyAdmitted { .. })
    }

    /// The value the site resolved to, for the verdicts that carry one.
    pub fn resolved(&self) -> Option<&str> {
        match self {
            Self::NewlyAdmitted { resolved } | Self::NotARoute { resolved } => Some(resolved),
            _ => None,
        }
    }
}

/// One site's verdict, and the per-operand key outcomes it was reached from —
/// kept so a census line can show the evidence, not just the conclusion.
pub struct Judgement {
    /// Parallel to the site's operands; `None` where the operand folds from the
    /// source and needs no configuration.
    pub outcomes: Vec<Option<KeyOutcome>>,
    pub verdict: Verdict,
}

/// The judgement both arms share: resolve every operand that does not fold,
/// take the agreement of each resolved key, and compose what the site would
/// resolve to.
///
/// `route_required` is the one arm-specific input, and it decides two things:
/// a client call must compose a route the arm can bind ([FR-WS-08]) and may
/// spend a trailing unresolvable operand as the `{}` a route template already
/// expresses, while a broker topic is admitted on its value alone
/// ([FR-WS-10]) and every one of its operands must resolve — a topic with a
/// `{}` in it is not a topic.
pub fn judge(
    nodes: &[Node<'_>],
    kinds: &[OperandKind],
    src: &[u8],
    unit: &Unit<'_>,
    resolver: Resolver<'_>,
    route_required: bool,
) -> Judgement {
    // Resolution is attempted on every operand that does **not** fold — not
    // only on the ones S-355's taxonomy labelled `configuration lookup`.
    //
    // That label is a *name* heuristic (`looks_like_configuration`'s needle
    // list), and on this corpus it under-reads by a wide margin: the broker
    // arm's beans are called `KafkaTopics`, reached through a field called
    // `kafkaTopics`, which contains none of the needles — so a first pass over
    // 54 header-form publish sites offered a denominator of 3. Resolution here
    // is **type-driven** (does the receiver's declared type name an indexed
    // `@ConfigurationProperties` class?), which is stronger evidence than the
    // spelling of a field. The taxonomy's own subset is still reported, since
    // it is the denominator [CR-115]'s acceptance criterion names.
    let mut outcomes: Vec<Option<KeyOutcome>> = Vec::with_capacity(nodes.len());
    for (node, kind) in nodes.iter().zip(kinds) {
        outcomes.push((!kind.is_foldable()).then(|| resolve_key(*node, src, unit, resolver)));
    }
    // Already admitted: a single static literal needs nothing from this change.
    // Shares the parent's predicate rather than restating it — two spellings of
    // "already admitted" could disagree and each publish a different figure.
    if super::is_already_static_literal(kinds) {
        return Judgement { outcomes, verdict: Verdict::AlreadyAdmitted };
    }
    if outcomes.iter().all(Option::is_none) {
        return Judgement { outcomes, verdict: Verdict::NotConfigurationBound };
    }

    // An operand that resolves to no key at all is fatal only where it MUST
    // resolve: the leading one always (an unknown prefix is not a resolved
    // site, [CR-113] §3.2 inherited), and every one of them on the broker arm
    // (a topic with a `{}` in it is not a topic). A *trailing* one on the
    // client arm is the `{}` placeholder a route template already expresses —
    // refusing it would have under-counted `getUriX() + id` sites, which is
    // what the fixture below caught.
    let fatal = outcomes.iter().enumerate().find_map(|(i, outcome)| match outcome {
        Some(KeyOutcome::Unresolved(refusal)) if i == 0 || !route_required => Some(*refusal),
        _ => None,
    });
    if let Some(refusal) = fatal {
        return Judgement { outcomes, verdict: Verdict::NoKey(refusal) };
    }

    // An operand that DOES resolve to a key must agree, wherever it sits. A
    // conflicting configuration value is not a route parameter: turning it into
    // `{}` because it happens to be trailing would be the default-profile guess
    // [CR-115] §3.4 exists to refuse, wearing a different hat.
    // An environment variable is not a committed source, so its scope is
    // decided before its agreement: [CR-115] §3.3 excludes it whatever the
    // sources say, and `canonical_key` lower-cases and strips separators, so
    // `getenv("BASE_URL")` would otherwise collide with a yml `base-url` and be
    // counted in the headline as though the repository proved it.
    if outcomes
        .iter()
        .flatten()
        .any(|o| matches!(o, KeyOutcome::Resolved { source: KeySource::Environment, .. }))
    {
        return Judgement { outcomes, verdict: Verdict::OutOfScopeSource };
    }

    let agreements: Vec<Option<Agreement>> = outcomes
        .iter()
        .map(|o| o.as_ref().and_then(KeyOutcome::key).map(|k| resolver.agreement(k)))
        .collect();
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Missing)) {
        return Judgement { outcomes, verdict: Verdict::MissingKey };
    }
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Placeholder { .. })) {
        return Judgement { outcomes, verdict: Verdict::PlaceholderValue };
    }
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Disagreed { .. })) {
        return Judgement { outcomes, verdict: Verdict::Disagreement };
    }
    // Nothing resolved through configuration: the composition folds from the
    // source alone, or from a trailing placeholder. Either way S-355 already
    // measured it and this arm must not re-count it as its own recovery.
    if !outcomes.iter().flatten().any(|o| o.key().is_some()) {
        // ...unless a configuration accessor was recognised and simply could
        // not be read. Position decided this before: a LEADING unresolved
        // lookup was fatal and counted, a TRAILING one fell through to
        // `NotConfigurationBound`, which `Tally::add` drops from `denominator`
        // and `report_census` hides. The same refusal was in or out of the
        // published denominator purely by where it sat in the composition —
        // shrinking the denominator and so inflating the recovered share the
        // verdict turns on. [CR-115] AC1 also requires every such site be
        // reported with its reason.
        let unread_lookup = kinds
            .iter()
            .zip(&outcomes)
            .find_map(|(kind, outcome)| match outcome {
                Some(KeyOutcome::Unresolved(refusal))
                    if *kind == OperandKind::ConfigurationLookup =>
                {
                    Some(*refusal)
                }
                _ => None,
            });
        let verdict = match unread_lookup {
            Some(refusal) => Verdict::NoKey(refusal),
            None => Verdict::NotConfigurationBound,
        };
        return Judgement { outcomes, verdict };
    }

    // Every configuration operand agrees. Compose what the site resolves to:
    // a configuration value, a folded same-unit constant or literal, or the
    // `{}` placeholder a route template already expresses.
    let Some(resolved) = compose(nodes, &agreements, src, unit, route_required) else {
        return Judgement { outcomes, verdict: Verdict::NoKey(Refusal::UnrecognisedAccessor) };
    };
    let verdict = if !route_required || binds_a_route(&resolved) {
        Verdict::NewlyAdmitted { resolved }
    } else {
        Verdict::NotARoute { resolved }
    };
    Judgement { outcomes, verdict }
}

/// The text the site resolves to. Returns `None` when the **leading** operand
/// resolves to nothing — a composition whose prefix is unknown is not resolved
/// at all, whatever its tail says ([CR-113] §3.2, inherited by [CR-115]).
///
/// Shared with the parent module's S-355 headline, which passes an all-`None`
/// `agreements` slice: with no configuration values in play this is exactly
/// [FR-WS-18] AC1's placeholder rule. The two headlines rest on one
/// implementation of that rule so a change to it cannot make them mean
/// different things.
pub(super) fn compose(
    nodes: &[Node<'_>],
    agreements: &[Option<Agreement>],
    src: &[u8],
    unit: &Unit<'_>,
    allow_placeholders: bool,
) -> Option<String> {
    let mut out = String::new();
    for (i, node) in nodes.iter().enumerate() {
        let text = agreements
            .get(i)
            .and_then(Option::as_ref)
            .and_then(Agreement::value)
            .map(str::to_string)
            .or_else(|| folded_text(*node, src, unit, FOLD_DEPTH));
        match text {
            Some(text) => out.push_str(&text),
            None if i == 0 => return None,
            None if allow_placeholders => out.push_str("{}"),
            None => return None,
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// Whether a resolved client-call template names a route the arm can bind: an
/// absolute path, or an absolute URL carrying one.
///
/// [CR-115] §3.4 is explicit that the *host* need not resolve —
/// `http://pec-anagrafica/api/v1` names a service-discovery target and binding
/// matches on the portable route key — so an absolute URL is admitted here,
/// unlike in the S-355 folding measurement where no configuration value existed
/// to supply one.
pub fn binds_a_route(template: &str) -> bool {
    if template.starts_with('/') {
        return true;
    }
    let Some((scheme, rest)) = template.split_once("://") else {
        return false;
    };
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return false;
    }
    rest.split_once('/').is_some_and(|(host, path)| !host.is_empty() && !path.is_empty())
}

// ── The broker-publish arm ──────────────────────────────────────────────────

/// The message-header publish form, as a harness-local query.
///
/// Deliberately shape-only: the header constant and the method name are
/// filtered in Rust rather than by query predicates.
///
/// Not because predicates do not work — `tree_sitter` 0.25 *does* evaluate the
/// text predicates (`#eq?`, `#match?`, `#any-of?`) inside `QueryMatches`, which
/// is why `count_broker_captures` can rely on the real `brokers.scm`'s
/// `#any-of?` firing. The reason is that this arm's filter is not a text
/// predicate at all: it accepts four different spellings of the topic header
/// (qualified, statically imported, and the wire name) and must stay readable
/// beside `names_topic_header`, which the fixtures pin directly.
const HEADER_PUBLISH_QUERY: &str = r"
(method_invocation
  name: (identifier) @publish.method
  arguments: (argument_list
    . (_) @publish.header
    . (_) @publish.topic))
";

/// The Spring Kafka topic header, in the two spellings the corpus uses: the
/// constant (qualified or statically imported) and its wire name.
fn names_topic_header(text: &str) -> bool {
    let text = text.trim();
    text == "KafkaHeaders.TOPIC"
        || text.ends_with(".KafkaHeaders.TOPIC")
        || text == "TOPIC"
        || text.trim_matches('"') == "kafka_topic"
}

/// One classified broker publish site.
#[derive(Debug, Clone)]
pub struct BrokerSite {
    pub file: String,
    pub line: u32,
    pub text: String,
    pub kinds: Vec<OperandKind>,
    pub outcomes: Vec<Option<KeyOutcome>>,
    pub verdict: Verdict,
    /// The topic operand is already a static literal at the header-form site.
    /// Recognising the form ([CR-117] §3.2 / S-370) admits it on its own; the
    /// configuration rule is not what unlocks it, so it is excluded from this
    /// measurement's newly-admitted count and reported separately.
    pub literal_topic: bool,
}

/// Per-language broker figures.
#[derive(Debug, Default)]
pub struct BrokerStats {
    pub files_scanned: usize,
    /// What the real `brokers.scm` captures today, for the denominator.
    pub publish_literals_today: usize,
    pub subscribe_literals_today: usize,
    /// Whether the header form is even expressible in this language's grammar.
    pub header_form_supported: bool,
    pub sites: Vec<BrokerSite>,
}

/// Compile the header-form query against a language, or `None` when the
/// grammar has no such node shape (every non-Java grammar in the set).
pub fn header_publish_query(language: &tree_sitter::Language) -> Option<tree_sitter::Query> {
    tree_sitter::Query::new(language, HEADER_PUBLISH_QUERY).ok()
}

/// Count what the real `brokers.scm` captures in this file — the arm's own
/// output, not the harness's reading of it.
pub fn count_broker_captures(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    stats: &mut BrokerStats,
) {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "broker.publish.topic" => stats.publish_literals_today += 1,
                "broker.subscribe.topic" => stats.subscribe_literals_today += 1,
                _ => {}
            }
        }
    }
}

/// Collect and judge every message-header publish site in one file.
pub fn collect_header_publishes(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    rel: &str,
    resolver: Resolver<'_>,
) -> Vec<BrokerSite> {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method = None;
        let mut header = None;
        let mut topic = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "publish.method" => method = Some(cap.node),
                "publish.header" => header = Some(cap.node),
                "publish.topic" => topic = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method), Some(header), Some(topic)) = (method, header, topic) else {
            continue;
        };
        if method.utf8_text(src).unwrap_or_default().trim() != "setHeader" {
            continue;
        }
        if !names_topic_header(header.utf8_text(src).unwrap_or_default()) {
            continue;
        }
        let mut nodes = Vec::new();
        super::operands(topic, src, &mut nodes);
        let kinds: Vec<OperandKind> =
            nodes.iter().map(|n| super::classify(*n, src, unit, FOLD_DEPTH)).collect();
        let literal_topic = static_literal(topic, src).is_some();
        let judgement = judge(&nodes, &kinds, src, unit, resolver, false);
        out.push(BrokerSite {
            file: rel.to_string(),
            line: method.start_position().row as u32 + 1,
            text: topic
                .utf8_text(src)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            kinds,
            outcomes: judgement.outcomes,
            verdict: judgement.verdict,
            literal_topic,
        });
    }
    out
}

// ── The base-URL half of CR-115 ─────────────────────────────────────────────

/// Builder methods that set the base a client's paths compose against.
const BASE_URL_METHODS: [&str; 4] = ["baseUrl", "baseURL", "setBaseUrl", "rootUri"];

/// A call with at least one argument. The method name is filtered in Rust, as
/// everywhere else here, because query predicates are not evaluated.
const BASE_URL_QUERY: &str = r"
(method_invocation
  name: (identifier) @base.method
  arguments: (argument_list . (_) @base.arg))
";

/// One `.baseUrl(…)` site and what its operand resolves to.
#[derive(Debug, Clone)]
pub struct BaseUrlSite {
    pub file: String,
    pub line: u32,
    pub text: String,
    pub outcome: KeyOutcome,
    pub agreement: Agreement,
}

pub fn base_url_query(language: &tree_sitter::Language) -> Option<tree_sitter::Query> {
    tree_sitter::Query::new(language, BASE_URL_QUERY).ok()
}

/// Collect the base-URL sites of one file.
///
/// Why this is measured at all: [CR-115] is titled *base-URL* resolution, and
/// its §3.4 worked example is a host that varies per profile. The client arm's
/// headline counts **path** operands, because that is what [FR-WS-08] binds a
/// route key on — §3.4 says in as many words that the host need not resolve.
/// Those are two different questions and the second one is the one most likely
/// to disagree, so it is reported rather than folded into the first.
pub fn collect_base_urls(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    rel: &str,
    resolver: Resolver<'_>,
) -> Vec<BaseUrlSite> {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method = None;
        let mut arg = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "base.method" => method = Some(cap.node),
                "base.arg" => arg = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method), Some(arg)) = (method, arg) else { continue };
        let name = method.utf8_text(src).unwrap_or_default().trim();
        if !BASE_URL_METHODS.contains(&name) {
            continue;
        }
        // A base URL the unit itself proves — a literal, or a same-unit
        // constant folding to one — needs no configuration source and cannot
        // disagree with one. `sources: 0` records that it was proven at the
        // call site rather than by the corpus.
        let folded = folded_text(arg, src, unit, FOLD_DEPTH);
        let (outcome, agreement) = match folded {
            Some(value) => (
                // An empty key is the sentinel `report_base_urls` reads as
                // "proven at the call site". `KeySource::CallSite` names it
                // honestly: recording it as `@Value` was simply false.
                KeyOutcome::Resolved {
                    key: String::new(),
                    source: KeySource::CallSite,
                    declared_in: None,
                },
                Agreement::Agreed { value, sources: 0 },
            ),
            None => {
                let outcome = resolve_key(arg, src, unit, resolver);
                let agreement = match &outcome {
                    KeyOutcome::Resolved { key, .. } => resolver.agreement(key),
                    KeyOutcome::Unresolved(_) => Agreement::Missing,
                };
                (outcome, agreement)
            }
        };
        out.push(BaseUrlSite {
            file: rel.to_string(),
            line: method.start_position().row as u32 + 1,
            text: arg
                .utf8_text(src)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            outcome,
            agreement,
        });
    }
    out
}

// ── Reporting ───────────────────────────────────────────────────────────────

/// One arm's tally, in the shape both arms and the combined line share.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tally {
    /// **The materiality denominator**: sites the arm refuses today whose
    /// composition needs a value the source does not hold — every site this
    /// mechanism could conceivably admit, however the operand is spelled.
    pub denominator: usize,
    /// **The acceptance-criterion denominator**: the subset of those sites
    /// S-355's taxonomy labelled a *configuration lookup*. Reported because
    /// [CR-115]'s criterion is phrased over it, and because the gap between the
    /// two is itself a finding — the taxonomy is a name heuristic and the
    /// broker arm's beans are named in a way it does not catch.
    pub config_labelled: usize,
    /// Newly admitted **within** that labelled subset — the figure
    /// [CR-115]'s acceptance criterion asks for, as distinct from the total.
    pub newly_admitted_labelled: usize,
    pub newly_admitted: usize,
    pub disagreement: usize,
    pub missing_key: usize,
    pub placeholder: usize,
    pub no_key: usize,
    pub not_a_route: usize,
    /// Resolved, but from an environment variable — [CR-115] §3.3 out of scope.
    pub out_of_scope: usize,
    pub already_admitted: usize,
}

impl Tally {
    fn add(&mut self, verdict: &Verdict, kinds: &[OperandKind]) {
        if matches!(verdict, Verdict::AlreadyAdmitted) {
            self.already_admitted += 1;
            return;
        }
        if matches!(verdict, Verdict::NotConfigurationBound) {
            return;
        }
        self.denominator += 1;
        if kinds.contains(&OperandKind::ConfigurationLookup) {
            self.config_labelled += 1;
        }
        let labelled = kinds.contains(&OperandKind::ConfigurationLookup);
        match verdict {
            Verdict::NewlyAdmitted { .. } => {
                self.newly_admitted += 1;
                if labelled {
                    self.newly_admitted_labelled += 1;
                }
            }
            Verdict::Disagreement => self.disagreement += 1,
            Verdict::MissingKey => self.missing_key += 1,
            Verdict::PlaceholderValue => self.placeholder += 1,
            Verdict::NoKey(_) => self.no_key += 1,
            Verdict::NotARoute { .. } => self.not_a_route += 1,
            Verdict::OutOfScopeSource => self.out_of_scope += 1,
            Verdict::AlreadyAdmitted | Verdict::NotConfigurationBound => unreachable!(),
        }
    }

    fn merge(&mut self, other: &Self) {
        self.denominator += other.denominator;
        self.config_labelled += other.config_labelled;
        self.newly_admitted_labelled += other.newly_admitted_labelled;
        self.newly_admitted += other.newly_admitted;
        self.disagreement += other.disagreement;
        self.missing_key += other.missing_key;
        self.placeholder += other.placeholder;
        self.no_key += other.no_key;
        self.not_a_route += other.not_a_route;
        self.out_of_scope += other.out_of_scope;
        self.already_admitted += other.already_admitted;
    }

    /// Whether the mechanism recovers enough of its own denominator to be worth
    /// building, against the floors declared before the run.
    pub fn is_material(&self) -> bool {
        self.newly_admitted >= MATERIAL_FLOOR_SITES
            && self.denominator > 0
            && self.newly_admitted * 100 >= self.denominator * MATERIAL_FLOOR_PCT
    }

    /// The recovered share of the arm's own denominator, in whole percent.
    pub fn percent(&self) -> usize {
        (self.newly_admitted * 100).checked_div(self.denominator).unwrap_or(0)
    }

    fn header() -> String {
        format!(
            "{:<12} {:>6} {:>7} {:>6} {:>8} {:>7} {:>8} {:>6} {:>9} {:>4} {:>6}",
            "language",
            "denom",
            "cfg-lbl",
            "NEW",
            "disagree",
            "missing",
            "placehld",
            "no-key",
            "not-route",
            "env",
            "literal",
        )
    }

    fn row(&self, label: &str) -> String {
        format!(
            "{:<12} {:>6} {:>7} {:>6} {:>8} {:>7} {:>8} {:>6} {:>9} {:>4} {:>6}",
            label,
            self.denominator,
            self.config_labelled,
            self.newly_admitted,
            self.disagreement,
            self.missing_key,
            self.placeholder,
            self.no_key,
            self.not_a_route,
            self.out_of_scope,
            self.already_admitted,
        )
    }
}

/// Whether a site lives in production source or in a test tree.
///
/// This exists because the arm split alone hid an inversion. The broker arm
/// reported 38 of 54 admitted — and **every one of the 38 was an IT test
/// class**, while all 13 sites in `src/main` were refused. A cross-service
/// coupling edge derived from a test fixture is not the coupling [CR-117] is
/// about, and [CR-117] §6's acceptance criterion is written over the
/// main-source population specifically. The story's own rule — "a material
/// figure for one and an immaterial figure for the other must not be averaged
/// away" — applies one level deeper than the two arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tree {
    Main,
    Test,
}

impl Tree {
    /// Classify by path. Deliberately broad: Maven/Gradle `src/test`, a
    /// top-level `test`/`tests` directory, Go's `_test.go`, and the `IT`/`Test`
    /// class-name suffixes the corpus uses.
    pub fn of(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        let in_test_dir = lower.contains("/src/test/")
            || lower.starts_with("src/test/")
            || lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.starts_with("test/")
            || lower.starts_with("tests/");
        let test_named = lower.ends_with("_test.go")
            || lower.ends_with("it.java")
            || lower.ends_with("test.java")
            || lower.ends_with("tests.java")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".spec.ts");
        if in_test_dir || test_named {
            Self::Test
        } else {
            Self::Main
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Test => "test",
        }
    }
}

/// Both arms' figures, separately and combined — the object the verdict is read
/// off, so the two are never averaged into one. Each arm is additionally split
/// by [`Tree`], because a yield that lives entirely in test source is a
/// different verdict from the same yield in production source.
#[derive(Debug, Default)]
pub struct Verdicts {
    pub client: BTreeMap<String, Tally>,
    pub broker: BTreeMap<String, Tally>,
    /// Per arm ("client-call" / "broker"), per tree.
    pub by_tree: BTreeMap<(&'static str, Tree), Tally>,
}

impl Verdicts {
    pub fn client_total(&self) -> Tally {
        let mut total = Tally::default();
        for t in self.client.values() {
            total.merge(t);
        }
        total
    }

    pub fn broker_total(&self) -> Tally {
        let mut total = Tally::default();
        for t in self.broker.values() {
            total.merge(t);
        }
        total
    }

    /// The combined figure. Reported **alongside** the two arms, never instead
    /// of them: [CR-115] and [CR-117] are decided on their own arm.
    pub fn combined(&self) -> Tally {
        let mut total = self.client_total();
        total.merge(&self.broker_total());
        total
    }
}

/// Print the S-365 measurement and return both arms' figures.
pub fn report(m: &super::Measurement) -> Verdicts {
    println!(
        "\n=== S-365: configuration-key resolvability and profile agreement ===\n\
         \nOne gate, two arms. CR-115 §3.4's rule is applied verbatim to both: a\
         \nkey is resolved only when EVERY committed source that defines it agrees\
         \non the value. The arms are reported separately and combined — never\
         \naveraged — because CR-115 and CR-117 are decided on their own figure.\n"
    );
    report_sources(m);
    let mut verdicts = Verdicts::default();
    report_client_arm(m, &mut verdicts);
    report_broker_arm(m, &mut verdicts);
    report_newly_admitted(m);
    report_base_urls(m);
    let (workspace_agreed, headline_total) = workspace_scope_headline(m);
    report_totals(&verdicts);
    println!(
        "  workspace-scope check: of the {headline_total} sites the module-scoped rule admits, \
         {workspace_agreed} also agree\n  across EVERY source in the workspace — CR-115 §3.4 read \
         strictly admits {workspace_agreed}, not {headline_total}."
    );
    report_refusals(m);
    report_census(m);
    verdicts
}

fn report_sources(m: &super::Measurement) {
    let corpus = &m.config;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for source in &corpus.sources {
        let ext = source.path.rsplit('.').next().unwrap_or("?");
        *by_kind.entry(ext).or_default() += 1;
    }
    let keys: BTreeSet<&String> =
        corpus.sources.iter().flat_map(|s| s.values.keys()).collect();
    let profiles = corpus.profiles();
    println!("--- committed configuration sources (FR-SY-11 admission) ---");
    println!(
        "{} sources, {} distinct keys, {} unprofiled + {} profiled",
        corpus.sources.len(),
        keys.len(),
        corpus.sources.iter().filter(|s| s.profile.is_none()).count(),
        corpus.sources.iter().filter(|s| s.profile.is_some()).count(),
    );
    for (ext, count) in &by_kind {
        println!("  application*.{ext:<12} {count:>4}");
    }
    println!(
        "  profiles discovered: {}",
        if profiles.is_empty() {
            "none".to_string()
        } else {
            profiles.into_iter().collect::<Vec<_>>().join(", ")
        },
    );
    println!(
        "  @ConfigurationProperties classes indexed: {} ({} annotated but prefixless, \
         {} simple-name collisions refused)",
        m.properties.len(),
        m.properties.prefixless,
        m.properties.collisions.len(),
    );
}

fn report_client_arm(m: &super::Measurement, verdicts: &mut Verdicts) {
    println!(
        "\n--- ARM 1: client-call sites (CR-115) ---\n\
         `denom`   every gate-admitted `invocations` site the arm refuses today whose\n\
         .         composition needs a value the source does not hold. The materiality\n\
         .         denominator: what this mechanism could conceivably admit.\n\
         `cfg-lbl` the subset S-355's taxonomy labelled `configuration lookup` — the\n\
         .         denominator CR-115's acceptance criterion is phrased over (its 81\n\
         .         Java sites). The two differ, so both are printed.\n\
         `literal` sites already a single static literal, excluded from `denom`.\n\
         `not-route` keys all agreed, but the composition still names no bindable\n\
         .         route (FR-WS-08 AC2).\n"
    );
    println!("{}", Tally::header());
    for (lang, stats) in &m.per_language {
        let mut tally = Tally::default();
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            tally.add(&site.cr115, &site.kinds);
            verdicts
                .by_tree
                .entry(("client-call", Tree::of(&site.file)))
                .or_default()
                .add(&site.cr115, &site.kinds);
        }
        println!("{}", tally.row(lang));
        verdicts.client.insert(lang.clone(), tally);
    }
    println!("{}", verdicts.client_total().row("ALL"));
}

fn report_broker_arm(m: &super::Measurement, verdicts: &mut Verdicts) {
    println!(
        "\n--- ARM 2: broker publish sites (CR-117 §3.3) ---\n\
         Denominator: every message-header publish site — `setHeader(KafkaHeaders.TOPIC,\n\
         …)` — whose topic operand is not a literal. A DIFFERENT denominator from arm 1\n\
         and not comparable to it: this arm has no ledger gate, and the real\n\
         `brokers.scm` recognises the header form not at all, so nothing here is\n\
         admitted today. Note how far `cfg-lbl` falls below `denom`: the corpus's topic\n\
         beans are called `KafkaTopics`, a name S-355's `looks_like_configuration`\n\
         heuristic does not catch, which is why resolution here is type-driven.\n"
    );
    println!("{}", Tally::header());
    for (lang, stats) in &m.broker {
        let mut tally = Tally::default();
        for site in &stats.sites {
            tally.add(&site.verdict, &site.kinds);
            verdicts
                .by_tree
                .entry(("broker", Tree::of(&site.file)))
                .or_default()
                .add(&site.verdict, &site.kinds);
        }
        println!("{}", tally.row(lang));
        verdicts.broker.insert(lang.clone(), tally);
    }
    println!("{}", verdicts.broker_total().row("ALL"));
    println!("\nwhat the real `brokers.scm` sees today, and what the header form adds:");
    for (lang, stats) in &m.broker {
        let literal_header_sites = stats.sites.iter().filter(|s| s.literal_topic).count();
        println!(
            "{lang:<12} {:>5} files; today publish/subscribe literals {}/{}; \
             header-form sites {} ({} with a literal topic, admitted by recognition alone); \
             header form expressible in this grammar: {}",
            stats.files_scanned,
            stats.publish_literals_today,
            stats.subscribe_literals_today,
            stats.sites.len(),
            literal_header_sites,
            stats.header_form_supported,
        );
    }
}

/// The strict [CR-115] §3.4 reading — "every discovered source **in the
/// workspace** that defines the key agrees" — recomputed over the same sites.
///
/// The headline is module-scoped, because a Maven module is the classpath one
/// deployable actually assembles and two members may legitimately bind one key
/// differently. But §3.4's words are workspace-wide, and on this corpus the two
/// differ, so the report must state which produced the headline and what the
/// other would have given.
fn workspace_scope_headline(m: &super::Measurement) -> (usize, usize) {
    let client = m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.gate_admitted && s.cr115.is_newly_admitted())
        .map(|s| &s.key_outcomes);
    let broker = m
        .broker
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.verdict.is_newly_admitted())
        .map(|s| &s.outcomes);

    let mut total = 0usize;
    let mut agreed = 0usize;
    for outcomes in client.chain(broker) {
        total += 1;
        let holds = outcomes.iter().flatten().all(|outcome| match outcome.key() {
            Some(key) => matches!(m.config.agreement(key, None), Agreement::Agreed { .. }),
            None => true,
        });
        if holds {
            agreed += 1;
        }
    }
    (agreed, total)
}

fn report_totals(verdicts: &Verdicts) {
    let client = verdicts.client_total();
    let broker = verdicts.broker_total();
    let combined = verdicts.combined();
    println!(
        "\n--- both arms, separately and combined (never averaged) ---\n{}",
        Tally::header()
    );
    println!("{}", client.row("client-call"));
    println!("{}", broker.row("broker"));
    println!("{}", combined.row("COMBINED"));
    println!(
        "\n--- each arm split by PRODUCTION vs TEST source ---\n\
         The arm split alone is not enough. A yield that lives entirely in test fixtures is\n\
         a different verdict from the same yield in production code, and CR-117 §6's\n\
         acceptance criterion is written over the main-source population specifically.\n"
    );
    println!("{}", Tally::header());
    for arm in ["client-call", "broker"] {
        for tree in [Tree::Main, Tree::Test] {
            let tally = verdicts.by_tree.get(&(arm, tree)).copied().unwrap_or_default();
            println!("{}", tally.row(&format!("{arm}/{}", tree.label())));
        }
    }
    println!(
        "\nSCOPE: the headline is MODULE-scoped — agreement is taken over the sources under\n\
         the call site's own build module, which is the classpath one deployable assembles.\n\
         CR-115 §3.4's words are workspace-wide; that stricter reading is printed beside it\n\
         so the CR is decided on the reading it means."
    );
    println!(
        "\nmateriality floor, declared before the run: >= {MATERIAL_FLOOR_SITES} sites AND \
         >= {MATERIAL_FLOOR_PCT}% of the arm's own denominator"
    );
    for (name, tally, cr) in [
        ("client-call", client, "CR-115"),
        ("broker", broker, "CR-117"),
    ] {
        let main = verdicts.by_tree.get(&(name, Tree::Main)).copied().unwrap_or_default();
        println!(
            "  {name:<12} {} newly admitted / {} refused-today sites = {}%  \
             [taxonomy-labelled subset: {} of {}]",
            tally.newly_admitted,
            tally.denominator,
            tally.percent(),
            tally.newly_admitted_labelled,
            tally.config_labelled,
        );
        println!(
            "  {:<12}   of which in PRODUCTION source: {} of {} = {}%  ->  {cr} CRA-01 {}",
            "",
            main.newly_admitted,
            main.denominator,
            main.percent(),
            if main.is_material() { "HOLDS" } else { "FALSIFIED" },
        );
        if tally.is_material() != main.is_material() {
            println!(
                "  {:<12}   ** the whole-arm figure and the production figure DISAGREE. \
                 The production one decides {cr}: an edge derived from a test fixture is not \
                 the coupling it is about. **",
                "",
            );
        }
    }
}

/// The headline figure, listed site by site. A count nobody can check is not
/// evidence, and this is the count both change requests turn on.
fn report_newly_admitted(m: &super::Measurement) {
    println!("\n--- the headline: every site the agreement rule would NEWLY admit ---");
    let mut listed = 0usize;
    for (lang, stats) in &m.per_language {
        for site in stats.sites.iter().filter(|s| s.gate_admitted && s.cr115.is_newly_admitted()) {
            listed += 1;
            println!("client  {lang}  {}:{}  {}  ->  {}", site.file, site.line, site.text, site.cr115.resolved().unwrap_or_default());
        }
    }
    for (lang, stats) in &m.broker {
        for site in stats.sites.iter().filter(|s| s.verdict.is_newly_admitted()) {
            listed += 1;
            println!("broker  {lang}  {}:{}  {}  ->  {}", site.file, site.line, site.text, site.verdict.resolved().unwrap_or_default());
        }
    }
    if listed == 0 {
        println!("  (none)");
    }
}

/// [CR-115]'s other half: the base the admitted paths compose against.
fn report_base_urls(m: &super::Measurement) {
    println!(
        "\n--- CR-115's OTHER half: the base URL those paths compose against ---\n\
         The headline above counts PATH operands, because FR-WS-08 binds a route key on\n\
         the path and CR-115 §3.4 states the host need not resolve. Whether the host\n\
         *agrees* is a different question, and on this corpus it is the one that fails.\n\
         Both figures are given so the CR is decided on the reading it actually means.\n"
    );
    let sites: Vec<&BaseUrlSite> = m.base_urls.values().flatten().collect();
    fn agreed(site: &BaseUrlSite) -> bool {
        matches!(site.agreement, Agreement::Agreed { .. })
    }
    let resolved_to_a_key = sites
        .iter()
        .filter(|s| s.outcome.key().is_some_and(|k| !k.is_empty()))
        .count();
    let proven_at_the_call_site =
        sites.iter().filter(|s| s.outcome.key() == Some("")).count();
    println!(
        "{} base-URL sites; {resolved_to_a_key} resolved to a configuration key, \
         {proven_at_the_call_site} proven at the call site;\nagreed {}; disagreed {}; \
         no source defines the key {}; accessor unresolved {}",
        sites.len(),
        sites.iter().filter(|s| agreed(s)).count(),
        sites.iter().filter(|s| matches!(s.agreement, Agreement::Disagreed { .. })).count(),
        sites
            .iter()
            .filter(|s| matches!(s.agreement, Agreement::Missing) && s.outcome.key().is_some())
            .count(),
        sites.iter().filter(|s| matches!(s.outcome, KeyOutcome::Unresolved(_))).count(),
    );
    let agreeing_files: BTreeSet<&str> =
        sites.iter().filter(|s| agreed(s)).map(|s| s.file.as_str()).collect();
    let base_url_files: BTreeSet<&str> = sites.iter().map(|s| s.file.as_str()).collect();
    let (mut with, mut against, mut none) = (0usize, 0usize, 0usize);
    for site in m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.gate_admitted && s.cr115.is_newly_admitted())
    {
        if agreeing_files.contains(site.file.as_str()) {
            with += 1;
        } else if base_url_files.contains(site.file.as_str()) {
            against += 1;
        } else {
            none += 1;
        }
    }
    println!(
        "\nof the newly-admitted client-call sites: {with} sit in a unit whose base URL \
         ALSO agrees,\n{against} in a unit whose base URL does not resolve or does not \
         agree, and\n{none} in a unit that sets no base URL of its own (it is configured \
         elsewhere).\n\
         \nSo the STRICTER reading of CR-115 — the whole absolute URL must be proven — \
         admits {with},\nand the FR-WS-08 route-key reading admits the headline figure. \
         Both are on the record."
    );
    for site in sites.iter().filter(|s| !agreed(s)) {
        // An unresolved accessor is named by its refusal, not lumped under
        // `missing key`: "we could not read it" and "no source defines it" are
        // different faults and only the second is CR-115's rule biting.
        let why = match &site.outcome {
            KeyOutcome::Unresolved(refusal) => refusal.label(),
            KeyOutcome::Resolved { .. } => site.agreement.label(),
        };
        println!("  {}:{}  {}  ->  {why}", site.file, site.line, site.text);
    }
}

fn report_refusals(m: &super::Measurement) {
    let mut counts: BTreeMap<Refusal, usize> = BTreeMap::new();
    let outcomes = m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter().filter(|s| s.gate_admitted))
        .flat_map(|s| s.key_outcomes.iter())
        .chain(m.broker.values().flat_map(|s| s.sites.iter()).flat_map(|s| s.outcomes.iter()));
    for outcome in outcomes.flatten() {
        if let KeyOutcome::Unresolved(refusal) = outcome {
            *counts.entry(*refusal).or_default() += 1;
        }
    }
    println!("\n--- why an accessor did not resolve to a key (both arms) ---");
    for refusal in Refusal::ALL {
        println!("  {:<38} {:>4}", refusal.label(), counts.get(&refusal).copied().unwrap_or(0));
    }
}

fn report_census(m: &super::Measurement) {
    println!("\n--- client-call census: every configuration-bound site, auditable ---");
    for (lang, stats) in &m.per_language {
        for site in stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.cr115 != Verdict::NotConfigurationBound)
        {
            println!(
                "{lang}  {}:{}  {}  ->  {}{}",
                site.file,
                site.line,
                site.text,
                site.cr115.label(),
                describe_keys(&site.key_outcomes, &m.config, m.config.module_of(&site.file)),
            );
        }
    }
    println!("\n--- broker census: every message-header publish site, auditable ---");
    for (lang, stats) in &m.broker {
        for site in &stats.sites {
            let kinds: Vec<&str> = site.kinds.iter().map(|k| k.label()).collect();
            println!(
                "{lang}  {}:{}  [{}]  {}  ->  {}{}",
                site.file,
                site.line,
                kinds.join(" + "),
                site.text,
                site.verdict.label(),
                describe_keys(&site.outcomes, &m.config, m.config.module_of(&site.file)),
            );
        }
    }
}

/// The per-operand key trace appended to a census line: the key, the scope's
/// agreement, and — when the sources disagree — the conflicting files by name,
/// which is what [CR-115] §3.4's refusal is required to report.
fn describe_keys(outcomes: &[Option<KeyOutcome>], corpus: &ConfigCorpus, module: &str) -> String {
    let mut out = String::new();
    for outcome in outcomes.iter().flatten() {
        match outcome {
            KeyOutcome::Unresolved(refusal) => {
                out.push_str(&format!("\n      <unresolved: {}>", refusal.label()));
            }
            KeyOutcome::Resolved { key, declared_in, source } => {
                let scoped = corpus.agreement(key, Some(module));
                let workspace = corpus.agreement(key, None);
                out.push_str(&format!(
                    "\n      {key}  via {}  module: {}  workspace: {}{}",
                    source.label(),
                    scoped.detail(),
                    workspace.detail(),
                    declared_in
                        .as_deref()
                        .map(|f| format!("  declared in {f}"))
                        .unwrap_or_default(),
                ));
                if let Agreement::Disagreed { values } = &scoped {
                    for (value, files) in values {
                        out.push_str(&format!("\n        {value:?} <- {}", files.join(", ")));
                    }
                }
            }
        }
    }
    out
}

// ── The measurement ─────────────────────────────────────────────────────────

/// S-365 itself. Skips — loudly — when no corpus is configured, exactly as the
/// S-355 measurement it extends does.
#[test]
fn measure_configuration_agreement_over_the_reference_workspace() {
    let Some(root) = super::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-365 measurement (see this module's docs for the recorded finding)."
        );
        return;
    };
    let m = super::measurement(&root);
    let verdicts = report(m);
    let client = verdicts.client_total();
    let broker = verdicts.broker_total();
    println!("\n--- recorded finding ---\n{RECORDED_FINDING}");

    assert!(
        !m.config.sources.is_empty(),
        "the corpus at {} yielded no application.{{yml,yaml,properties}} source, so no \
         agreement was measured — refusing to report a green run that measured nothing",
        root.display(),
    );
    // The materiality floor (5 sites / 10%) cannot notice a collapsed harness:
    // a broker arm that regressed to 6 sites of which 5 admit still reads
    // "material". These guard the ORDER OF MAGNITUDE the finding was recorded
    // at, loosely enough to survive corpus churn.
    assert!(
        client.denominator >= 100 && broker.denominator >= 40,
        "the recorded finding measured 140 client / 54 broker refused-today sites; this run \
         saw {} / {}. A collapsed denominator is a broken harness, not a new finding.",
        client.denominator,
        broker.denominator,
    );
    assert!(
        !m.base_urls.is_empty(),
        "the CR-115 base-URL arm found no `.baseUrl(…)` site, so half of what CR-115 is \
         titled after was not measured at all",
    );
    assert!(
        m.config.profiles().len() >= 2,
        "a corpus with fewer than two profiles cannot exercise profile disagreement, which \
         is the rule CR-115 §3.4 is judged on — this run measured agreement that could not \
         have failed",
    );
    assert!(
        m.per_language
            .values()
            .flat_map(|s| s.sites.iter())
            .any(|s| s.gate_admitted && s.cr115 == Verdict::Disagreement),
        "no site anywhere was refused for disagreement, so the agreement rule's refusal path \
         was never taken. A run that never refuses is not evidence the rule works — and a \
         regression that stopped producing Disagreed would make every arm look MORE material",
    );
    assert!(
        !m.properties.is_empty(),
        "the corpus at {} declares no @ConfigurationProperties class, so the accessor \
         hop was never exercised and every client-call refusal would be \
         `no @ConfigurationProperties class` by construction",
        root.display(),
    );

    // The verdict is what blocks CR-115 and CR-117, so it is asserted rather
    // than printed. Both arms are pinned independently: a change that flipped
    // one and not the other must fail here, not average out.
    //
    // The verdict is read off PRODUCTION source, per arm. The whole-arm figure
    // is printed beside it but does not decide anything: the broker arm's 38
    // admits are all IT test classes and its 13 production sites yield zero, so
    // a whole-arm reading would have recorded CR-117 CRA-01 as holding on the
    // strength of test fixtures.
    let client_main =
        verdicts.by_tree.get(&("client-call", Tree::Main)).copied().unwrap_or_default();
    let broker_main = verdicts.by_tree.get(&("broker", Tree::Main)).copied().unwrap_or_default();

    assert!(
        client_main.is_material(),
        "S-365's recorded finding is that CR-115 CRA-01 HOLDS: the agreement rule newly \
         admits a MATERIAL number of PRODUCTION client-call sites. This run admitted {} of \
         {} ({}%), below the floor of {MATERIAL_FLOOR_SITES} sites and {MATERIAL_FLOOR_PCT}% \
         declared before the measurement was taken. That is a falsification, not a broken \
         test: mark CR-115 CRA-01 falsified with this run's evidence and date, leave S-366 \
         and S-367 unplanned, and re-decide the CR before changing this assertion.",
        client_main.newly_admitted,
        client_main.denominator,
        client_main.percent(),
    );
    assert!(
        !broker_main.is_material(),
        "S-365's recorded finding is that CR-117 CRA-01 is FALSIFIED on production source: \
         all {} of the broker arm's production publish sites are refused, and every site the \
         rule admits is an IT test class. This run admitted {} of {} production sites ({}%), \
         which CLEARS the declared floor — the corpus or the harness has changed. Re-read \
         CR-117 §8 CRA-01 and re-decide the CR before changing this assertion; a genuine \
         production yield would make S-371 plannable again.",
        broker_main.denominator,
        broker_main.newly_admitted,
        broker_main.denominator,
        broker_main.percent(),
    );

    // The verdicts are pinned per arm and never on the combined figure: a
    // material arm must not rescue an immaterial one. The fixture
    // `an_immaterial_arm_is_not_rescued_by_a_material_one` pins that property
    // of `is_material` itself; this is the corpus-level application of it.
    assert!(
        verdicts.combined().newly_admitted == client.newly_admitted + broker.newly_admitted,
        "the combined figure must be the sum of the arms, not an average of them",
    );
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// These run on every `cargo test`, corpus or no corpus. They matter for the
// same reason the parent module's do: the measurement above skips without a
// corpus, so without them this file would pin nothing in CI — and the numbers
// it produces are what unblock (or close) [CR-115] and [CR-117].
//
// Every fixture drives the real Java grammar and goes through `judge`, the same
// entry point both corpus arms use.

#[cfg(test)]
mod fixtures {
    use super::*;
    use logos_core::plugin::LanguageRegistry;
    use tree_sitter::Parser;

    /// A corpus of exactly one Java compilation unit plus the configuration
    /// sources it is bound from.
    struct Fixture {
        corpus: ConfigCorpus,
        props: PropertiesIndex,
    }

    impl Fixture {
        /// `sources` are `(filename, body)` pairs placed at the corpus root, so
        /// every one of them is in the same (root) module scope.
        fn new(classes: &[&str], sources: &[(&str, &str)]) -> Self {
            let mut corpus = ConfigCorpus::default();
            for (name, body) in sources {
                let values = if name.ends_with(".properties") {
                    parse_properties(body)
                } else {
                    parse_yaml(body)
                };
                corpus.sources.push(ConfigSource {
                    path: (*name).to_string(),
                    profile: config_profile(name).flatten(),
                    module: String::new(),
                    values,
                });
            }
            let mut props = PropertiesIndex::default();
            let language = java_language();
            let mut parser = Parser::new();
            parser.set_language(&language).expect("java language");
            for (i, class) in classes.iter().enumerate() {
                let tree = parser.parse(class, None).expect("parse");
                props.absorb(&format!("Props{i}.java"), "", tree.root_node(), class.as_bytes());
            }
            props.seal();
            Self { corpus, props }
        }

        /// Judge the single `.uri(…)` argument of a Java unit against this
        /// fixture's configuration.
        fn judge_uri(&self, unit_source: &str) -> Verdict {
            self.judge_expression(unit_source, true)
        }

        /// Judge it as a broker topic instead — no route requirement.
        fn judge_topic(&self, unit_source: &str) -> Verdict {
            self.judge_expression(unit_source, false)
        }

        fn judge_expression(&self, unit_source: &str, route_required: bool) -> Verdict {
            let language = java_language();
            let mut parser = Parser::new();
            parser.set_language(&language).expect("java language");
            let tree = parser.parse(unit_source, None).expect("parse");
            let src = unit_source.as_bytes();
            let unit = Unit::build(tree.root_node(), src);
            let arg = sole_probe_argument(tree.root_node(), src);
            let mut nodes = Vec::new();
            super::super::operands(arg, src, &mut nodes);
            let kinds: Vec<OperandKind> = nodes
                .iter()
                .map(|n| super::super::classify(*n, src, &unit, FOLD_DEPTH))
                .collect();
            let resolver =
                Resolver { corpus: &self.corpus, props: &self.props, module: "" };
            judge(&nodes, &kinds, src, &unit, resolver, route_required).verdict
        }
    }

    fn java_language() -> tree_sitter::Language {
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
        registry.for_path("Probe.java").expect("java plugin").language().clone()
    }

    /// The argument of the single `probe(…)` call a fixture unit must contain.
    /// A dedicated marker rather than `.uri(…)`, so a fixture never depends on
    /// the client-call query's verb gate to find its own expression.
    fn sole_probe_argument<'t>(root: tree_sitter::Node<'t>, src: &[u8]) -> tree_sitter::Node<'t> {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
            drop(cursor);
            if node.kind() != "method_invocation" {
                continue;
            }
            let is_probe = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .is_some_and(|n| n == "probe");
            if !is_probe {
                continue;
            }
            let args = node.child_by_field_name("arguments").expect("argument list");
            let mut cursor = args.walk();
            let first = args.named_children(&mut cursor).next().expect("one argument");
            drop(cursor);
            found.push(first);
        }
        assert_eq!(found.len(), 1, "a fixture unit must contain exactly one probe(…) call");
        found[0]
    }

    const PROPS: &str = r#"
        @ConfigurationProperties(prefix = "mailserver.api")
        public class MailServerConfigurationApi {
            private String baseUrl;
            private String uriGetArchive;
        }
    "#;

    const TOPICS: &str = r#"
        @ConfigurationProperties(prefix = "spring.kafka.topics")
        public class KafkaTopics {
            private String archiveCommands;
        }
    "#;

    fn unit(body: &str) -> String {
        format!("public class Client {{\n{body}\n}}\n")
    }

    // ── relaxed binding and value canonicalisation ──────────────────────────

    #[test]
    fn relaxed_binding_makes_the_three_spellings_one_key() {
        let canonical = canonical_key("mailserver.api.uri-get-archive");
        assert_eq!(canonical, canonical_key("mailserver.api.uriGetArchive"));
        assert_eq!(canonical, canonical_key("mailserver.api.URI_GET_ARCHIVE"));
        assert_eq!(canonical, "mailserver.api.urigetarchive");
    }

    #[test]
    fn relaxed_binding_does_not_merge_distinct_segments() {
        assert_ne!(canonical_key("a.b.c"), canonical_key("a.bc"));
    }

    #[test]
    fn yaml_flattens_nested_scalars_and_honours_document_separators() {
        let flat = parse_yaml(
            "mailserver:\n  api:\n    base-url: http://localhost:8000\n    \
             uri-get-archive: /x/{id}\n---\nother: 1\n",
        );
        assert_eq!(
            flat.get("mailserver.api.baseurl").map(|v| v.iter().next().unwrap().as_str()),
            Some("http://localhost:8000"),
        );
        assert_eq!(
            flat.get("mailserver.api.urigetarchive").map(|v| v.iter().next().unwrap().as_str()),
            Some("/x/{id}"),
        );
        // The separator reset the indent stack, so `other` is top-level.
        assert!(flat.contains_key("other"), "got {flat:?}");
    }

    #[test]
    fn yaml_reads_a_sequence_valued_key_as_absent_rather_than_as_a_scalar() {
        // Under-reading is the safe direction: it can only lower the
        // newly-admitted count, never inflate it.
        let flat = parse_yaml("topics:\n  - a\n  - b\nplain: v\n");
        assert!(!flat.contains_key("topics"));
        assert!(flat.contains_key("plain"));
    }

    #[test]
    fn yaml_skips_a_block_scalar_body_without_swallowing_the_next_key() {
        let flat = parse_yaml("banner: |\n  line one\n  line two\nnext: v\n");
        assert!(!flat.contains_key("banner"));
        assert_eq!(flat.get("next").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
    }

    #[test]
    fn a_blank_line_does_not_end_a_block_scalar_early() {
        let flat = parse_yaml("banner: |\n  one\n\n  key: not-a-key\nnext: v\n");
        assert!(!flat.contains_key("banner.key"), "got {flat:?}");
        assert!(!flat.contains_key("key"), "got {flat:?}");
        assert_eq!(flat.get("next").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
    }

    #[test]
    fn a_flow_collection_is_not_read_as_a_scalar_value() {
        // Mis-reading `topics: [a, b]` as the string "[a, b]" would let a
        // list-valued key be reported as agreed on a value it does not have.
        let flat = parse_yaml("topics: [a, b]\nmapping: {k: v}\nplain: p\n");
        assert!(!flat.contains_key("topics"), "got {flat:?}");
        assert!(!flat.contains_key("mapping"), "got {flat:?}");
        assert!(flat.contains_key("plain"));
    }

    #[test]
    fn properties_read_both_separators_comments_and_continuations() {
        let flat = parse_properties(
            "# comment\n! also a comment\na.b=1\nc.d: 2\ne.f=one\\\ntwo\n",
        );
        assert_eq!(flat.get("a.b").map(|v| v.iter().next().unwrap().as_str()), Some("1"));
        assert_eq!(flat.get("c.d").map(|v| v.iter().next().unwrap().as_str()), Some("2"));
        assert_eq!(flat.get("e.f").map(|v| v.iter().next().unwrap().as_str()), Some("onetwo"));
    }

    #[test]
    fn a_quoted_value_loses_its_quotes_and_an_unquoted_one_loses_its_trailing_comment() {
        let flat = parse_yaml("a: \"/x\"\nb: /y # why\nc: '/z'\n");
        for (key, want) in [("a", "/x"), ("b", "/y"), ("c", "/z")] {
            assert_eq!(
                flat.get(key).map(|v| v.iter().next().unwrap().as_str()),
                Some(want),
                "key {key}",
            );
        }
    }

    #[test]
    fn a_binding_cycle_terminates_instead_of_recursing_forever() {
        // `a = b; b = a` binds two DISTINCT ast nodes, so the identity check
        // alone does not terminate — FOLD_DEPTH is what does.
        let f = Fixture::new(&[PROPS], &[("application.yml", "x: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { String a = b; String b = a; probe(a); }"
            )),
            Verdict::NoKey(Refusal::UnrecognisedAccessor),
        );
    }

    #[test]
    fn a_binding_chain_still_reaches_the_configuration_accessor_behind_it() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert!(f
            .judge_uri(&unit(
                "private MailServerConfigurationApi mailServerConfigurationApi;\n\
                 void go() { String p = mailServerConfigurationApi.getUriGetArchive(); probe(p); }"
            ))
            .is_newly_admitted());
    }

    #[test]
    fn a_line_whose_key_is_not_one_token_registers_no_key() {
        let flat = parse_yaml("real-key: v\nsome prose: with a colon\n");
        assert!(flat.contains_key("realkey"));
        assert_eq!(flat.len(), 1, "got {flat:?}");
    }

    #[test]
    fn a_value_containing_a_colon_keeps_its_whole_value() {
        let flat = parse_yaml("url: http://host:8080/p\n");
        assert_eq!(
            flat.get("url").map(|v| v.iter().next().unwrap().as_str()),
            Some("http://host:8080/p"),
        );
    }

    // ── Regression fixtures: every defect the S-365 review found ───────────
    //
    // Each of these failed before its fix. They are grouped because they share
    // a property: all were invisible to the 46 fixtures that preceded them, and
    // most moved a published number.

    #[test]
    fn a_commented_mapping_header_is_not_read_as_a_scalar() {
        // Was: `api:   # the mail API` recorded `mailserver.api => "# the mail
        // API"` AND re-parented the whole subtree, so the real key vanished.
        let flat = parse_yaml(
            "mailserver:\n  api:   # the mail API\n    uri-get-archive: /a\n",
        );
        assert_eq!(
            flat.get("mailserver.api.urigetarchive").map(|v| v.iter().next().unwrap().as_str()),
            Some("/a"),
            "the subtree must stay under its header; got {flat:?}",
        );
        assert!(!flat.contains_key("mailserver.api"), "got {flat:?}");
    }

    #[test]
    fn a_sequence_items_own_keys_do_not_become_the_parents() {
        // Was: `- name: primary` was skipped but the item's CONTINUATION lines
        // registered at the parent path, fabricating a real-looking key from a
        // list element.
        let flat = parse_yaml(
            "mailserver:\n  api:\n    - name: primary\n      uri-get-archive: /wrong\n",
        );
        assert!(!flat.contains_key("mailserver.api.urigetarchive"), "got {flat:?}");
        assert!(!flat.contains_key("mailserver.api.name"), "got {flat:?}");
    }

    #[test]
    fn a_sequence_does_not_swallow_the_key_that_follows_it() {
        let flat = parse_yaml("list:\n  - a: 1\n    b: 2\nafter: v\n");
        assert_eq!(flat.get("after").map(|v| v.iter().next().unwrap().as_str()), Some("v"));
    }

    #[test]
    fn a_quoted_value_with_a_trailing_comment_loses_both() {
        // Was: the quote test required the string to END with the quote, which
        // a trailing comment defeats, so the value kept its quotes — a false
        // disagreement against an unquoted definition elsewhere.
        let flat = parse_yaml("a: \"/x\" # why\nb: '/y' # why\n");
        assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("/x"));
        assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("/y"));
    }

    #[test]
    fn a_value_may_contain_a_hash_that_is_not_a_comment() {
        let flat = parse_yaml("frag: \"/x#anchor\"\n");
        assert_eq!(flat.get("frag").map(|v| v.iter().next().unwrap().as_str()), Some("/x#anchor"));
    }

    #[test]
    fn every_block_scalar_indicator_skips_its_body() {
        // Was: only the four bare spellings were caught, so `|+` and `|2` were
        // recorded as VALUES and their bodies parsed as YAML — a bogus agreed
        // value plus phantom keys lifted out of the block.
        for indicator in ["|", ">", "|-", ">-", "|+", ">+", "|2"] {
            let flat = parse_yaml(&format!("banner: {indicator}\n  one\n  key: v\nnext: n\n"));
            assert!(!flat.contains_key("banner"), "{indicator}: got {flat:?}");
            assert!(!flat.contains_key("key"), "{indicator}: leaked a block line: {flat:?}");
            assert_eq!(
                flat.get("next").map(|v| v.iter().next().unwrap().as_str()),
                Some("n"),
                "{indicator}: lost the key after the block",
            );
        }
    }

    #[test]
    fn an_anchor_is_a_mapping_header_not_a_value() {
        let flat = parse_yaml("defaults: &d\n  url: /a\n");
        assert_eq!(
            flat.get("defaults.url").map(|v| v.iter().next().unwrap().as_str()),
            Some("/a"),
            "got {flat:?}",
        );
        assert!(!flat.contains_key("defaults"), "got {flat:?}");
    }

    #[test]
    fn a_properties_value_is_not_put_through_the_yaml_rules() {
        // In a .properties file `#` is not an inline comment and quotes are
        // literal. Borrowing the YAML rules changed the value Java would read,
        // which can make two sources falsely agree.
        let flat = parse_properties("a=/x # main\nb=\"/y\"\n");
        assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("/x # main"));
        assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("\"/y\""));
    }

    #[test]
    fn a_properties_escaped_backslash_is_a_value_not_a_continuation() {
        // Was: any trailing backslash continued the line, so `a=C:\\tmp\\`
        // swallowed the entry after it and `b` disappeared entirely.
        let flat = parse_properties("a=C:\\\\tmp\\\\\nb=2\n");
        assert!(flat.contains_key("b"), "the next entry was swallowed: {flat:?}");
        assert_eq!(flat.get("b").map(|v| v.iter().next().unwrap().as_str()), Some("2"));
    }

    #[test]
    fn a_properties_odd_backslash_still_continues() {
        let flat = parse_properties("a=one\\\ntwo\n");
        assert_eq!(flat.get("a").map(|v| v.iter().next().unwrap().as_str()), Some("onetwo"));
    }

    #[test]
    fn a_value_annotated_field_does_not_answer_for_a_parameter_that_shadows_it() {
        // Was: `Unit` is file-scoped, so the field's @Value key resolved for the
        // PARAMETER of the same name and the site counted as admitted. Field /
        // parameter name collision is idiomatic Spring.
        let f = Fixture::new(&[], &[("application.yml", "app:\n  path: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "@Value(\"${app.path}\") private String path;\n\
                 void go(String path) { probe(path); }"
            )),
            Verdict::NoKey(Refusal::MethodParameter),
            "the operand is the parameter, whose value a caller supplies",
        );
    }

    #[test]
    fn a_bean_getter_named_get_env_is_not_an_environment_read() {
        // Was: the needle `getenv(` was matched against the lower-cased whole
        // operand, so `configApi.getEnv("A")` was misread as an environment
        // read and pre-empted the properties lookup.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getEnv(\"A\")); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::PropertyNotDeclared),
            "this is a bean getter, not System.getenv",
        );
    }

    #[test]
    fn an_environment_read_nested_in_an_expression_does_not_fabricate_a_key() {
        // Was: the delimiter search anchored on the FIRST `(` in the operand,
        // so this yielded the key `System.getenv("HOST` — a fabricated key
        // replacing a real refusal reason.
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        let verdict = f.judge_uri(&unit(
            "void go() { probe(Optional.ofNullable(System.getenv(\"HOST\")).orElse(\"/x\")); }",
        ));
        assert!(
            matches!(verdict, Verdict::NoKey(_) | Verdict::NotConfigurationBound),
            "expected a refusal, got {verdict:?}",
        );
    }

    #[test]
    fn an_environment_variable_is_never_admitted_even_when_a_key_agrees() {
        // CR-115 §3.3 puts environment variables out of scope. `canonical_key`
        // lower-cases and strips separators, so getenv("BASE_URL") collides
        // with a yml `base-url`; without the scope rule that collision counted
        // in the headline as though the repository proved it.
        let f = Fixture::new(&[], &[("application.yml", "base-url: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(System.getenv(\"BASE_URL\")); }")),
            Verdict::OutOfScopeSource,
        );
    }

    #[test]
    fn a_method_parameter_of_a_class_is_not_a_declared_property() {
        // Was: the record-component arm fired for ANY formal_parameter in the
        // class body, so a helper's parameter became a declared property —
        // loosening PropertyNotDeclared and, where a source defined the same
        // key, admitting the site outright.
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        let body = r#"
            @ConfigurationProperties(prefix = "api")
            public class P { private String a; public void helper(String uriGetArchive) {} }
        "#;
        let tree = parser.parse(body, None).expect("parse");
        props.absorb("P.java", "", tree.root_node(), body.as_bytes());
        props.seal();
        let class = props.get("P", "").expect("indexed");
        assert!(class.properties.contains(&canonical_key("a")));
        assert!(
            !class.properties.contains(&canonical_key("uriGetArchive")),
            "a method parameter is not a bound property",
        );
    }

    #[test]
    fn a_nested_types_fields_are_not_the_outer_classs_properties() {
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        let body = r#"
            @ConfigurationProperties(prefix = "api")
            public class P { private String a; static class Inner { private String b; } }
        "#;
        let tree = parser.parse(body, None).expect("parse");
        props.absorb("P.java", "", tree.root_node(), body.as_bytes());
        props.seal();
        let class = props.get("P", "").expect("indexed");
        assert!(class.properties.contains(&canonical_key("a")));
        assert!(!class.properties.contains(&canonical_key("b")));
    }

    #[test]
    fn two_same_named_classes_in_one_module_resolve_to_nothing_rather_than_a_guess() {
        // The own-module lookup used to return the first match without the
        // collision test — the same guess the workspace lookup forbids.
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        for (file, prefix) in [("m/a/C.java", "one"), ("m/b/C.java", "two")] {
            let body = format!(
                "@ConfigurationProperties(prefix = \"{prefix}\")\npublic class C {{ private String x; }}"
            );
            let tree = parser.parse(&body, None).expect("parse");
            props.absorb(file, "m", tree.root_node(), body.as_bytes());
        }
        props.seal();
        assert!(props.get("C", "m").is_none(), "one module, two different C — must not guess");
    }

    #[test]
    fn a_name_bound_to_two_different_accessors_resolves_to_neither() {
        // `folded_text` in the parent refuses a name bound to two literals for
        // exactly this reason; resolution used to pick whichever the DFS
        // reached first, which is not even source order.
        let f = Fixture::new(
            &[PROPS, TOPICS],
            &[(
                "application.yml",
                "mailserver:\n  api:\n    uri-get-archive: /a\nspring:\n  kafka:\n                     topics:\n      archive-commands: ac\n",
            )],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "private MailServerConfigurationApi mailServerConfigurationApi;\n\
                 private KafkaTopics kafkaTopics;\n\
                 void go() { String u = mailServerConfigurationApi.getUriGetArchive(); \
                 u = kafkaTopics.getArchiveCommands(); probe(u); }"
            )),
            Verdict::NoKey(Refusal::AmbiguousBinding),
        );
    }

    #[test]
    fn a_refused_configuration_lookup_counts_wherever_it_sits_in_the_composition() {
        // Was: a LEADING unresolved lookup was fatal and counted; a TRAILING
        // one fell through to NotConfigurationBound, which Tally::add drops
        // from the denominator and the census hides. The same refusal was in
        // or out of the published denominator purely by position — shrinking
        // the denominator and so inflating the recovered share.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        let decl = "private MailServerConfigurationApi mailServerConfigurationApi;";
        let leading = f.judge_uri(&unit(&format!(
            "{decl}\nvoid go() {{ probe(mailServerConfigurationApi.getUriPutArchive() + \"/x\"); }}"
        )));
        let trailing = f.judge_uri(&unit(&format!(
            "{decl}\nvoid go() {{ probe(\"/x\" + mailServerConfigurationApi.getUriPutArchive()); }}"
        )));
        assert_eq!(leading, Verdict::NoKey(Refusal::PropertyNotDeclared));
        assert_eq!(
            trailing, leading,
            "position must not decide whether a refusal is counted",
        );
    }

    #[test]
    fn a_profile_variant_is_recognised_whole() {
        assert_eq!(config_profile("application.yml"), Some(None));
        assert_eq!(config_profile("application.yaml"), Some(None));
        assert_eq!(config_profile("application.properties"), Some(None));
        assert_eq!(
            config_profile("application-it-jenkins.yml"),
            Some(Some("it-jenkins".to_string())),
            "a hyphen inside the profile must not split it",
        );
        assert_eq!(config_profile("bootstrap.yml"), None);
        assert_eq!(config_profile("application.json"), None);
        assert_eq!(config_profile("application-.yml"), None);
    }

    // ── the agreement rule (CR-115 §3.4) ────────────────────────────────────

    #[test]
    fn one_source_defining_the_key_agrees_with_itself() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NewlyAdmitted { resolved: "/a/{id}".to_string() },
        );
    }

    #[test]
    fn several_sources_agreeing_still_admits() {
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
            ],
        );
        assert!(f
            .judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                              private MailServerConfigurationApi mailServerConfigurationApi;"))
            .is_newly_admitted());
    }

    #[test]
    fn profile_disagreement_refuses_rather_than_defaulting_to_the_unprofiled_value() {
        // The rule CR-115 §3.4 is judged on: no default-profile fallback.
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    uri-get-archive: /b/{id}\n"),
            ],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::Disagreement,
        );
    }

    #[test]
    fn a_key_no_source_defines_is_a_missing_key_not_a_disagreement() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "other: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::MissingKey,
        );
    }

    #[test]
    fn a_value_that_is_itself_a_placeholder_proves_nothing() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: ${ARCHIVE_URI}\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::PlaceholderValue,
        );
    }

    #[test]
    fn a_properties_file_and_a_yaml_file_disagreeing_is_still_a_disagreement() {
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n"),
                ("application.properties", "mailserver.api.uri-get-archive=/b\n"),
            ],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::Disagreement,
        );
    }

    // ── accessor resolution, and each way it fails ──────────────────────────

    #[test]
    fn the_getter_suffix_binds_the_relaxed_key() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        // Declared as `uriGetArchive` on the class, spelled `uri-get-archive`
        // in the yml, read as `getUriGetArchive()` at the call site.
        assert!(f
            .judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                              private MailServerConfigurationApi mailServerConfigurationApi;"))
            .is_newly_admitted());
    }

    #[test]
    fn a_qualified_this_receiver_resolves_like_a_bare_one() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert!(f
            .judge_uri(&unit(
                "void go() { probe(this.mailServerConfigurationApi.getUriGetArchive()); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            ))
            .is_newly_admitted());
    }

    #[test]
    fn a_getter_for_a_property_the_class_does_not_declare_is_refused() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-put-archive: /a\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriPutArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::PropertyNotDeclared),
        );
    }

    #[test]
    fn a_receiver_whose_type_declares_no_properties_class_is_refused() {
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(someBean.getUriGetArchive()); }\n\
                               private SomeBean someBean;")),
            Verdict::NoKey(Refusal::NoPropertiesClass),
        );
    }

    #[test]
    fn a_receiver_the_unit_never_declares_is_refused_for_its_type_not_its_class() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(undeclared.getUriGetArchive()); }")),
            Verdict::NoKey(Refusal::ReceiverTypeUnknown),
        );
    }

    #[test]
    fn a_chained_getter_is_refused_as_a_nested_accessor_not_guessed_through() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(config.getApi().getUriGetArchive()); }\n\
                               private MailServerConfigurationApi config;")),
            Verdict::NoKey(Refusal::NestedAccessor),
        );
    }

    #[test]
    fn a_non_getter_member_call_is_refused_as_such() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.resolve()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::NotAGetter),
        );
    }

    #[test]
    fn a_method_parameter_is_refused_as_one_call_frame_away() {
        // CR-117 CRA-04's residue, counted rather than mistaken for a defect.
        let f = Fixture::new(&[TOPICS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_topic(&unit("void send(String topic) { probe(topic); }")),
            Verdict::NoKey(Refusal::MethodParameter),
        );
    }

    #[test]
    fn a_value_annotated_field_resolves_in_one_hop_and_drops_its_default() {
        let f = Fixture::new(&[], &[("application.yml", "app:\n  path: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { probe(path); }\n\
                 @Value(\"${app.path:/fallback}\") private String path;"
            )),
            Verdict::NewlyAdmitted { resolved: "/from-yml".to_string() },
        );
    }

    #[test]
    fn an_environment_read_resolves_to_its_variable_and_is_reported_out_of_scope() {
        // Resolved on purpose — the census can then name the variable rather
        // than shrug at an "unreadable shape" — but never admitted: CR-115 §3.3
        // excludes environment variables because they are not committed.
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(System.getenv(\"API_HOST\")); }")),
            Verdict::OutOfScopeSource,
        );
    }

    // ── composition and the route rule ──────────────────────────────────────

    #[test]
    fn a_configuration_prefix_composes_with_a_literal_suffix() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { probe(mailServerConfigurationApi.getUriGetArchive() + \"/sub\"); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            )),
            Verdict::NewlyAdmitted { resolved: "/a/sub".to_string() },
        );
    }

    #[test]
    fn a_trailing_unresolvable_operand_becomes_the_placeholder_a_route_already_expresses() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/\n")],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "void go(String id) { probe(mailServerConfigurationApi.getUriGetArchive() + id); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            )),
            Verdict::NewlyAdmitted { resolved: "/a/{}".to_string() },
        );
    }

    #[test]
    fn a_leading_unresolvable_operand_refuses_the_whole_composition() {
        // CR-113 §3.2's rule, inherited: an unknown prefix is not a resolved
        // site whatever its tail says.
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        let verdict = f.judge_uri(&unit(
            "void go(String base) { probe(base + mailServerConfigurationApi.getUriGetArchive()); }\n\
             private MailServerConfigurationApi mailServerConfigurationApi;",
        ));
        assert_eq!(verdict, Verdict::NoKey(Refusal::MethodParameter));
    }

    #[test]
    fn an_absolute_url_binds_a_route_because_the_host_need_not_resolve() {
        // CR-115 §3.4 is explicit: `http://pec-anagrafica/api/v1` names a
        // service-discovery target and binding matches the portable route key.
        assert!(binds_a_route("http://pec-anagrafica/api/v1"));
        assert!(binds_a_route("https://host/p"));
        assert!(binds_a_route("/relative-to-root"));
        assert!(!binds_a_route("relative"));
        assert!(!binds_a_route("http://host-with-no-path"));
        assert!(!binds_a_route("://p"));
    }

    #[test]
    fn a_resolved_value_that_names_no_route_is_reported_as_such_not_as_admitted() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: relative\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NotARoute { resolved: "relative".to_string() },
        );
    }

    #[test]
    fn the_broker_arm_admits_a_topic_that_names_no_route() {
        // The one place the arms differ: FR-WS-10 binds a topic on its value,
        // FR-WS-08 binds a client call on a route.
        let f = Fixture::new(
            &[TOPICS],
            &[("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")],
        );
        assert_eq!(
            f.judge_topic(&unit("void go() { probe(kafkaTopics.getArchiveCommands()); }\n\
                                 private KafkaTopics kafkaTopics;")),
            Verdict::NewlyAdmitted { resolved: "ac".to_string() },
        );
    }

    #[test]
    fn a_topic_bean_named_without_a_configuration_needle_still_resolves() {
        // The defect the first corpus run exposed: S-355's taxonomy labels an
        // operand by the SPELLING of its receiver, and `kafkaTopics` contains
        // none of its needles. Resolution here is type-driven, so the label
        // does not gate it — a 54-site corpus must not report a denominator
        // of 3.
        let f = Fixture::new(
            &[TOPICS],
            &[("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")],
        );
        assert!(!looks_like_configuration_needle("kafkaTopics"), "premise of this test");
        assert!(f
            .judge_topic(&unit("void go() { probe(kafkaTopics.getArchiveCommands()); }\n\
                                private KafkaTopics kafkaTopics;"))
            .is_newly_admitted());
    }

    /// The parent module's name heuristic, reached through a local shim so the
    /// test above states its premise instead of assuming it.
    fn looks_like_configuration_needle(text: &str) -> bool {
        super::super::looks_like_configuration(text)
    }

    #[test]
    fn a_site_whose_every_operand_folds_is_outside_this_measurement() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "static final String P = \"/a\";\nvoid go() { probe(P + \"/b\"); }"
            )),
            Verdict::NotConfigurationBound,
            "S-355 already measured folding; S-365 must not re-count it",
        );
    }

    #[test]
    fn a_single_static_literal_is_already_admitted_not_newly() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(\"/a\"); }")),
            Verdict::AlreadyAdmitted,
        );
    }

    // ── the module scope, and the collision it exists to prevent ────────────

    #[test]
    fn a_use_site_prefers_the_properties_class_its_own_module_declares() {
        // The corpus defect: two members declare `MailServerConfigurationApi`
        // under one prefix with different property sets, and a workspace-wide
        // "first wins" index reported eleven false `property not declared`
        // refusals against the wrong member's class.
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        let narrow = r#"
            @ConfigurationProperties(prefix = "mailserver.api")
            public class MailServerConfigurationApi { private String uriGetArchive; }
        "#;
        let wide = r#"
            @ConfigurationProperties(prefix = "mailserver.api")
            public class MailServerConfigurationApi {
                private String uriGetArchive;
                private String uriUpdateArchive;
            }
        "#;
        for (module, body) in [("archive-api", narrow), ("archive-manager", wide)] {
            let tree = parser.parse(body, None).expect("parse");
            props.absorb(&format!("{module}/C.java"), module, tree.root_node(), body.as_bytes());
        }
        props.seal();

        assert!(
            props.collisions.contains("MailServerConfigurationApi"),
            "declarations that disagree must be recorded as a collision",
        );
        let own = props.get("MailServerConfigurationApi", "archive-manager").expect("own module");
        assert!(own.properties.contains(&canonical_key("uriUpdateArchive")));
        assert!(
            props.get("MailServerConfigurationApi", "unrelated-member").is_none(),
            "a colliding name must resolve to nothing rather than to a guess",
        );
    }

    #[test]
    fn identical_declarations_in_several_modules_are_not_a_collision() {
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        for module in ["a", "b"] {
            let tree = parser.parse(PROPS, None).expect("parse");
            props.absorb(&format!("{module}/C.java"), module, tree.root_node(), PROPS.as_bytes());
        }
        props.seal();
        assert!(props.collisions.is_empty());
        assert!(props.get("MailServerConfigurationApi", "c").is_some());
    }

    #[test]
    fn the_annotation_value_form_carries_a_prefix_too() {
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        let body = r#"
            @ConfigurationProperties("spring.datasource.batch")
            public class Ds { private String url; }
        "#;
        let tree = parser.parse(body, None).expect("parse");
        props.absorb("C.java", "", tree.root_node(), body.as_bytes());
        props.seal();
        assert_eq!(props.get("Ds", "").map(|c| c.prefix.as_str()), Some("spring.datasource.batch"));
    }

    #[test]
    fn a_record_declares_its_components_as_properties() {
        let mut props = PropertiesIndex::default();
        let language = java_language();
        let mut parser = Parser::new();
        parser.set_language(&language).expect("java language");
        let body = r#"
            @ConfigurationProperties(prefix = "api")
            public record ApiProps(String baseUrl, String uriGet) {}
        "#;
        let tree = parser.parse(body, None).expect("parse");
        props.absorb("C.java", "", tree.root_node(), body.as_bytes());
        props.seal();
        let class = props.get("ApiProps", "").expect("indexed");
        assert!(class.properties.contains(&canonical_key("baseUrl")));
        assert!(class.properties.contains(&canonical_key("uriGet")));
    }

    // ── the two arms are never averaged ─────────────────────────────────────

    // ── The accounting itself, which every published number passes through ─

    #[test]
    fn the_tally_counts_each_verdict_into_exactly_one_bucket() {
        // `Tally::add` was reached only by the corpus walk: both materiality
        // fixtures built `Tally` literals instead. A one-line change letting
        // `AlreadyAdmitted` fall through to `denominator += 1` would move every
        // percentage in the finding with nothing failing.
        let cfg = [OperandKind::ConfigurationLookup];
        let lit = [OperandKind::Literal];
        let other = [OperandKind::Other];
        let mut t = Tally::default();
        t.add(&Verdict::AlreadyAdmitted, &lit);
        t.add(&Verdict::NotConfigurationBound, &lit);
        t.add(&Verdict::NewlyAdmitted { resolved: "/a".into() }, &cfg);
        t.add(&Verdict::NewlyAdmitted { resolved: "/b".into() }, &other);
        t.add(&Verdict::Disagreement, &cfg);
        t.add(&Verdict::MissingKey, &cfg);
        t.add(&Verdict::PlaceholderValue, &cfg);
        t.add(&Verdict::NoKey(Refusal::UnboundName), &other);
        t.add(&Verdict::NotARoute { resolved: "x".into() }, &cfg);
        t.add(&Verdict::OutOfScopeSource, &cfg);

        assert_eq!(t.already_admitted, 1);
        assert_eq!(
            t.denominator, 8,
            "AlreadyAdmitted and NotConfigurationBound are excluded from the denominator",
        );
        assert_eq!(
            t.newly_admitted + t.disagreement + t.missing_key + t.placeholder + t.no_key
                + t.not_a_route + t.out_of_scope,
            t.denominator,
            "the buckets must partition the denominator exactly",
        );
        assert_eq!(t.config_labelled, 6);
        assert_eq!(t.newly_admitted, 2);
        assert_eq!(t.newly_admitted_labelled, 1, "the labelled subset is CR-115's criterion");
    }

    #[test]
    fn merge_carries_every_field() {
        let a = Tally {
            denominator: 1,
            config_labelled: 2,
            newly_admitted_labelled: 3,
            newly_admitted: 4,
            disagreement: 5,
            missing_key: 6,
            placeholder: 7,
            no_key: 8,
            not_a_route: 9,
            out_of_scope: 10,
            already_admitted: 11,
        };
        let mut b = a;
        b.merge(&a);
        assert_eq!(
            (
                b.denominator,
                b.config_labelled,
                b.newly_admitted_labelled,
                b.newly_admitted,
                b.disagreement,
                b.missing_key,
                b.placeholder,
                b.no_key,
                b.not_a_route,
                b.out_of_scope,
                b.already_admitted,
            ),
            (2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22),
            "a dropped line in `merge` silently under-reports a bucket",
        );
    }

    #[test]
    fn every_refusal_variant_is_listed_in_all_with_a_distinct_label() {
        // `report_refusals` iterates `ALL`; a variant added without extending
        // it disappears from the published refusal census silently.
        let labels: BTreeSet<&str> = Refusal::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(labels.len(), Refusal::ALL.len(), "two refusals share a label");
        // An exhaustive match: adding a variant fails to compile until `ALL`
        // and this list are both extended.
        for refusal in Refusal::ALL {
            match refusal {
                Refusal::NestedAccessor
                | Refusal::MethodParameter
                | Refusal::UnboundName
                | Refusal::AmbiguousBinding
                | Refusal::NotAGetter
                | Refusal::ReceiverTypeUnknown
                | Refusal::NoPropertiesClass
                | Refusal::PropertyNotDeclared
                | Refusal::UnrecognisedAccessor => {}
            }
        }
    }

    #[test]
    fn a_name_the_unit_never_binds_is_refused_as_unbound() {
        // The largest single refusal bucket in the published finding (41 of the
        // client arm's 59) had no fixture at all.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_topic(&unit("void go() { probe(neverBoundAnywhere); }")),
            Verdict::NoKey(Refusal::UnboundName),
        );
    }

    #[test]
    fn a_topic_with_a_trailing_unresolvable_operand_is_refused_because_it_is_not_a_topic() {
        // The one place the arms genuinely differ, and every broker fixture
        // passed a SINGLE operand, so `i == 0 || !route_required` was never
        // exercised on the `!route_required` half. Mutating it to `i == 0`
        // left every fixture green while turning refused multi-operand topics
        // into admitted ones — inflating exactly the number CR-117 turns on.
        let sources =
            [("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")];
        let f = Fixture::new(&[TOPICS], &sources);
        let source = unit(
            "private KafkaTopics kafkaTopics;\n\
             void send(String suffix) { probe(kafkaTopics.getArchiveCommands() + suffix); }",
        );
        assert_eq!(
            f.judge_topic(&source),
            Verdict::NoKey(Refusal::MethodParameter),
            "FR-WS-10 binds a topic on its value: a topic with a `{{}}` in it is not a topic",
        );
        assert_eq!(
            f.judge_uri(&source),
            Verdict::NotARoute { resolved: "ac{}".to_string() },
            "the client arm SPENDS the same trailing operand as a `{{}}` placeholder and \
             composes a template — it is then judged on the route shape, not refused at the \
             operand. (`ac{{}}` is no route, so this one still ends refused; what differs is \
             WHY.)",
        );
    }

    #[test]
    fn refusal_precedence_is_worst_fault_first_across_several_operands() {
        // Every previous fixture had exactly one configuration operand, so the
        // Missing -> Placeholder -> Disagreed ordering was never exercised.
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    base-url: /a\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    base-url: /b\n"),
            ],
        );
        // `uriGetArchive` is declared on the class but defined by no source
        // (missing); `baseUrl` is defined by two sources that disagree.
        assert_eq!(
            f.judge_uri(&unit(
                "private MailServerConfigurationApi c;\n\
                 void go() { probe(c.getBaseUrl() + c.getUriGetArchive()); }"
            )),
            Verdict::MissingKey,
            "a missing key is a deeper fault than a disagreement and must be reported first",
        );
    }

    #[test]
    fn a_literal_plus_a_parameter_is_still_s355_territory() {
        // The guard exists for exactly this shape: nothing is fatal, nothing
        // resolved to a key, and without it the site composes to `/a/{}`,
        // clears `binds_a_route` and inflates the CR-115 headline.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go(String id) { probe(\"/a/\" + id); }")),
            Verdict::NotConfigurationBound,
        );
    }

    #[test]
    fn the_production_and_test_trees_are_told_apart() {
        // The split that inverted CR-117's verdict.
        for main in [
            "archive-api/src/main/java/com/x/KafkaProducer.java",
            "internal/flow/handler.go",
            "web/src/app.ts",
        ] {
            assert_eq!(Tree::of(main), Tree::Main, "{main}");
        }
        for test in [
            "archive-api/src/test/java/com/x/KafkaProducerIT.java",
            "archive-api/src/main/java/com/x/FooTest.java",
            "internal/flow/handler_test.go",
            "tests/e2e/probe.java",
        ] {
            assert_eq!(Tree::of(test), Tree::Test, "{test}");
        }
    }

    #[test]
    fn an_immaterial_arm_is_not_rescued_by_a_material_one() {
        // The acceptance criterion this test exists for: "a material figure for
        // one and an immaterial figure for the other is a real possible outcome
        // and must not be averaged away".
        let mut verdicts = Verdicts::default();
        verdicts.client.insert(
            "java".to_string(),
            Tally { denominator: 100, newly_admitted: 90, ..Tally::default() },
        );
        verdicts.broker.insert(
            "java".to_string(),
            Tally { denominator: 100, newly_admitted: 1, ..Tally::default() },
        );
        assert!(verdicts.client_total().is_material());
        assert!(!verdicts.broker_total().is_material());
        assert!(
            verdicts.combined().is_material(),
            "the combined figure would clear the floor — which is exactly why the \
             per-arm verdicts, not the combined one, decide the change requests",
        );
    }

    #[test]
    fn the_materiality_floor_needs_both_a_share_and_a_count() {
        let tiny = Tally { denominator: 4, newly_admitted: 4, ..Tally::default() };
        assert!(!tiny.is_material(), "4 of 4 is 100% of almost nothing");
        let thin = Tally { denominator: 1000, newly_admitted: 50, ..Tally::default() };
        assert!(!thin.is_material(), "50 sites is 5% — below the declared share");
        let real = Tally { denominator: 100, newly_admitted: 50, ..Tally::default() };
        assert!(real.is_material());
    }

    #[test]
    fn a_zero_denominator_is_never_material() {
        assert!(!Tally::default().is_material());
        assert_eq!(Tally::default().percent(), 0);
    }
}
