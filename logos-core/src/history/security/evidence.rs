//! The evidence a suppression's reason rests on ([FR-SC-32], [S-818]).
//!
//! An ignore entry may carry **evidence**: graph symbols its reason rests on,
//! each recorded with a hash of the exact source span it had when the verdict
//! was made. A suppression keyed on a scanner's fingerprint lapses when the
//! matched lines change, but not when the code its *reason* relies on does ("the
//! cookie is Secure because `DefaultCookieConfig` sets it"); the evidence is
//! that second key. An entry suppresses only while every evidence symbol exists
//! and its span hash matches ([BR-102]); otherwise its finding counts again,
//! reading "verdict out of date", until the verdict is re-confirmed
//! ([`reconfirm`](super::suppress::reconfirm)).
//!
//! # The hash is not the graph node's `fingerprint`
//! `nodes.fingerprint` is the **normalised** AST-shape fingerprint clone
//! detection groups by ([`crate::extract`]'s `shape`, [FR-AN-02]): identifiers
//! collapse to one atom, comments and whitespace are skipped, and a literal
//! contributes its node kind only — so `Secure: true` and `Secure: false` have
//! the same fingerprint, and it is recorded for functions and methods alone. It
//! cannot tell a flipped literal, which is the change a verdict most needs to
//! notice. The evidence hash is a blake3 of the **exact bytes** of the symbol's
//! recorded line span, read from the file at the time of the check
//! (CRA-03, CRA-04): literals, comments and formatting all count, and the graph
//! supplies only *where* the symbol is. No stored column, no migration.
//!
//! # Honest about a graph that has not caught up
//! The span is the graph's, so it is trusted only while the file still holds
//! what the graph indexed (its stored content hash equals the file's). A file
//! that moved since the last index, or a graph that cannot be read, makes the
//! evidence *unverifiable*: the entry stops suppressing and says why, never
//! quietly holds ([NFR-CC-04]). A symbol the graph no longer knows is *gone*.
//!
//! [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md
//! [FR-AN-02]: ../../../../docs/specs/requirements/FR-AN-02.md
//! [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
//! [BR-102]: ../../../../docs/specs/software-spec.md#328-security-findings
//! [S-818]: ../../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::suppress::refused;
use crate::graph_store::{GraphStore, NodeRow, SqliteGraphStore};

/// The length of a recorded span hash: a blake3 digest in hex.
const HASH_LEN: usize = 64;

/// One symbol an entry's reason rests on, with the hash of its exact source
/// span when the verdict was made.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Evidence {
    /// The symbol's identity ([`NodeRow::identity`]): its id without the
    /// repository coordinate, so it survives a re-index and a fresh checkout.
    pub symbol: String,
    /// blake3, in hex, of the exact bytes of the symbol's line span.
    pub hash: String,
}

impl Evidence {
    /// This evidence checked, or why the file would reject it.
    pub(super) fn validated(self) -> std::result::Result<Self, String> {
        if self.symbol.trim().is_empty() {
            return Err("an evidence `symbol` is empty: name the symbol the reason rests on".to_string());
        }
        if self.symbol.trim() != self.symbol {
            return Err(format!(
                "the evidence symbol {:?} has leading or trailing space, so it could never name a symbol",
                self.symbol
            ));
        }
        if self.hash.len() != HASH_LEN || !self.hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!(
                "the evidence hash {:?} is not a span hash (expected {HASH_LEN} lowercase hex characters): record it with `logos security reconfirm`",
                self.hash
            ));
        }
        Ok(self)
    }
}

/// `evidence` in its one canonical form: each item checked, sorted by symbol, and
/// no symbol twice — what the file holds and what every writer records.
///
/// # Errors
/// Why the file would reject it: an item that is not a symbol with a span hash, or
/// a symbol named twice.
pub(super) fn canonical(evidence: Vec<Evidence>) -> std::result::Result<Vec<Evidence>, String> {
    let mut checked = evidence.into_iter().map(Evidence::validated).collect::<std::result::Result<Vec<_>, _>>()?;
    checked.sort();
    match checked.windows(2).find(|pair| pair[0].symbol == pair[1].symbol) {
        Some(twice) => Err(format!("the evidence names `{}` twice", twice[0].symbol)),
        None => Ok(checked),
    }
}

/// A recorded evidence symbol as a surface names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceSymbol {
    /// The identity the entry records.
    pub symbol: String,
    /// What a person calls it: `DefaultCookieConfig`.
    pub name: String,
}

impl EvidenceSymbol {
    /// The symbol an evidence record names.
    pub fn of(evidence: &Evidence) -> Self {
        Self { symbol: evidence.symbol.clone(), name: short_name(&evidence.symbol) }
    }
}

/// The name a person calls a symbol by, read from its identity: the last
/// descriptor, without its kind suffix. `` pkg/`f.go`/DefaultCookieConfig(). ``
/// is `DefaultCookieConfig`; an identity of no recognisable shape is its own name.
fn short_name(identity: &str) -> String {
    let tail = identity.rsplit("`/").next().unwrap_or(identity);
    let name = tail.trim_end_matches(['#', '.', '(', ')', ':', '!']).trim();
    if name.is_empty() { identity.to_string() } else { name.to_string() }
}

/// What reading one evidence symbol's span found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanReading {
    /// The hash of the span's exact bytes now.
    Hash(String),
    /// The symbol is no longer in the index, or its file is gone.
    Gone,
    /// The span cannot be trusted: the graph cannot be read, or the file moved
    /// since the index read it. The text says why.
    Unverifiable(String),
}

/// Where an evidence symbol's span hash is read from: the graph and the working
/// tree, or a fixture.
pub trait EvidenceSource {
    /// The span hash of the symbol recorded as `symbol` (an identity).
    fn read(&self, symbol: &str) -> SpanReading;
}

/// A source with no graph at all: nothing can be verified, so every symbol is
/// unverifiable — evidence a caller cannot check never suppresses ([BR-102]).
///
/// [BR-102]: ../../../../docs/specs/software-spec.md#328-security-findings
pub struct NoGraph;

impl EvidenceSource for NoGraph {
    fn read(&self, _symbol: &str) -> SpanReading {
        SpanReading::Unverifiable("no index to check it against".to_string())
    }
}

/// How one evidence symbol no longer supports its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Its span's bytes differ from the recorded ones.
    Changed,
    /// The symbol no longer exists.
    Gone,
    /// It could not be checked.
    Unverifiable,
}

/// One evidence symbol that stopped supporting its verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceChange {
    /// The identity the entry records.
    pub symbol: String,
    /// What a person calls it.
    pub name: String,
    /// What happened to it.
    pub change: ChangeKind,
    /// Why it could not be checked; `Some` exactly for [`ChangeKind::Unverifiable`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl EvidenceChange {
    /// `` `DefaultCookieConfig` changed `` — the clause a surface appends to
    /// "verdict out of date".
    pub fn describe(&self) -> String {
        match (&self.change, &self.detail) {
            (ChangeKind::Changed, _) => format!("`{}` changed", self.name),
            (ChangeKind::Gone, _) => format!("`{}` no longer exists", self.name),
            (ChangeKind::Unverifiable, Some(why)) => format!("`{}` could not be checked: {why}", self.name),
            (ChangeKind::Unverifiable, None) => format!("`{}` could not be checked", self.name),
        }
    }
}

/// The evidence that no longer holds, in the entry's order; empty while every
/// symbol exists with the span it was recorded with.
pub(super) fn out_of_date(evidence: &[Evidence], source: &dyn EvidenceSource) -> Vec<EvidenceChange> {
    evidence
        .iter()
        .filter_map(|recorded| {
            let (change, detail) = match source.read(&recorded.symbol) {
                SpanReading::Hash(now) if now == recorded.hash => return None,
                SpanReading::Hash(_) => (ChangeKind::Changed, None),
                SpanReading::Gone => (ChangeKind::Gone, None),
                SpanReading::Unverifiable(why) => (ChangeKind::Unverifiable, Some(why)),
            };
            Some(EvidenceChange { symbol: recorded.symbol.clone(), name: short_name(&recorded.symbol), change, detail })
        })
        .collect()
}

/// The hash of lines `start..=end` (1-based) of `bytes`: the exact bytes,
/// line terminators included, so a literal, a comment or a line ending moves it.
fn span_hash(bytes: &[u8], start: usize, end: usize) -> String {
    let mut span = Vec::new();
    for line in bytes.split_inclusive(|b| *b == b'\n').skip(start.saturating_sub(1)).take(end + 1 - start) {
        span.extend_from_slice(line);
    }
    blake3::hash(&span).to_hex().to_string()
}

/// The graph at a project root, opened on first use, read-only: where evidence
/// symbols are found and the working tree the spans are cut from. A project that
/// records no evidence never opens it.
pub struct GraphEvidence {
    root: PathBuf,
    loaded: OnceCell<std::result::Result<Loaded, String>>,
    /// Each file's bytes, read once however many evidence symbols sit in it.
    files: RefCell<HashMap<String, Option<Vec<u8>>>>,
}

struct Loaded {
    store: SqliteGraphStore,
    /// The content hash the index recorded for each file.
    indexed: HashMap<String, Option<String>>,
}

impl GraphEvidence {
    /// The graph of the project at `root`; nothing is opened yet.
    pub fn at(root: &Path) -> Self {
        Self { root: root.to_path_buf(), loaded: OnceCell::new(), files: RefCell::new(HashMap::new()) }
    }

    fn loaded(&self) -> std::result::Result<&Loaded, String> {
        self.loaded
            .get_or_init(|| {
                let path = self.root.join(".logos").join("logos.db");
                if !path.exists() {
                    return Err("the project has no index: run `logos index`".to_string());
                }
                let store = SqliteGraphStore::open_readonly(&path).map_err(|err| format!("the index could not be read: {err:#}"))?;
                let indexed = store
                    .indexed_files()
                    .map_err(|err| format!("the index could not be read: {err:#}"))?
                    .into_iter()
                    .map(|file| (file.path, file.content_hash))
                    .collect();
                Ok(Loaded { store, indexed })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The span hash of `node` now.
    fn locate(&self, loaded: &Loaded, node: &NodeRow) -> SpanReading {
        let (Some(path), Some(start), Some(end)) = (node.file_path.as_deref(), node.start_line, node.end_line) else {
            return SpanReading::Unverifiable("the index records no source span for it".to_string());
        };
        let (Ok(start), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
            return SpanReading::Unverifiable("the index records no source span for it".to_string());
        };
        let mut files = self.files.borrow_mut();
        let read = files.entry(path.to_string()).or_insert_with(|| std::fs::read(self.root.join(path)).ok());
        let Some(bytes) = read.as_deref() else {
            // A file that is not there is gone; one that cannot be read is not.
            return if self.root.join(path).exists() {
                SpanReading::Unverifiable(format!("{path} could not be read"))
            } else {
                SpanReading::Gone
            };
        };
        let now = blake3::hash(bytes).to_hex().to_string();
        if loaded.indexed.get(path).and_then(Option::as_deref) != Some(now.as_str()) {
            return SpanReading::Unverifiable(format!("the index is behind {path}: run `logos sync {path}`"));
        }
        SpanReading::Hash(span_hash(bytes, start, end.max(start)))
    }

    /// Resolve `text` — a symbol identity, a canonical symbol or a bare name —
    /// to the one symbol it names and record its evidence now.
    ///
    /// # Errors
    /// A [`ConfigError`](crate::config::ConfigError) (exit 2) when the graph
    /// cannot be read, `text` names no symbol with a source span or more than
    /// one (listing them, never guessing), or the index is behind its file.
    pub fn pin(&self, text: &str) -> Result<Evidence> {
        let text = text.trim();
        let loaded = self.loaded().map_err(|why| refused(format!("evidence `{text}`: {why}")))?;
        let node = resolve(&loaded.store, text).map_err(refused)?;
        match self.locate(loaded, &node) {
            SpanReading::Hash(hash) => Ok(Evidence { symbol: node.identity().to_string(), hash }),
            SpanReading::Gone => Err(refused(format!("evidence `{text}`: its file is gone from the working tree"))),
            SpanReading::Unverifiable(why) => Err(refused(format!("evidence `{text}`: {why}"))),
        }
    }
}

impl EvidenceSource for GraphEvidence {
    fn read(&self, symbol: &str) -> SpanReading {
        let loaded = match self.loaded() {
            Ok(loaded) => loaded,
            Err(why) => return SpanReading::Unverifiable(why),
        };
        match crate::navigate::resolve_exact_or_identity(&loaded.store, symbol) {
            Ok(Some((node, _))) => self.locate(loaded, &node),
            Ok(None) => SpanReading::Gone,
            Err(err) => SpanReading::Unverifiable(format!("the index could not be read: {err:#}")),
        }
    }
}

/// The one symbol with a source span that `text` names: exactly, by identity or
/// canonical symbol, else by bare name — which must match one symbol.
fn resolve(store: &dyn GraphStore, text: &str) -> std::result::Result<NodeRow, String> {
    if text.is_empty() {
        return Err("an evidence symbol is empty".to_string());
    }
    let read = |err: anyhow::Error| format!("evidence `{text}`: the index could not be read: {err:#}");
    if let Some((node, _)) = crate::navigate::resolve_exact_or_identity(store, text).map_err(read)? {
        return Ok(node);
    }
    let mut named: Vec<NodeRow> = store
        .nodes_by_name(text)
        .map_err(read)?
        .into_iter()
        .filter(|node| node.start_line.is_some() && node.file_path.is_some() && !node.kind.is_doc())
        .collect();
    match named.len() {
        0 => Err(format!("evidence `{text}` names no symbol in the index")),
        1 => Ok(named.remove(0)),
        many => {
            let listed: Vec<String> = named.iter().take(5).map(|node| format!("`{}`", node.identity())).collect();
            Err(format!(
                "evidence `{text}` names {many} symbols ({}{}): name one by its identity",
                listed.join(", "),
                if many > 5 { ", …" } else { "" }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_name_is_the_last_descriptor_without_its_suffix() {
        assert_eq!(short_name("internal/config/`cookie.go`/DefaultCookieConfig()."), "DefaultCookieConfig");
        assert_eq!(short_name("logos-core/src/`suppress.rs`/IgnoreFile#"), "IgnoreFile");
        assert_eq!(short_name("plain"), "plain");
    }

    /// Lines are cut with their terminators, so a change to one byte of a
    /// literal — or to the line ending — moves the hash, and a line outside the
    /// span never does.
    #[test]
    fn a_span_hash_covers_exactly_the_span_bytes() {
        let text = b"head\nSecure: true,\ntail\n";
        let span = span_hash(text, 2, 2);
        assert_eq!(span, blake3::hash(b"Secure: true,\n").to_hex().to_string());
        assert_ne!(span, span_hash(b"head\nSecure: false,\ntail\n", 2, 2), "a flipped literal moves it");
        assert_ne!(span, span_hash(b"head\nSecure: true,\r\ntail\n", 2, 2), "a line ending moves it");
        assert_eq!(span, span_hash(b"HEAD\nSecure: true,\nTAIL\n", 2, 2), "a line outside the span does not");
    }

    #[test]
    fn evidence_that_is_not_a_span_hash_is_rejected() {
        let hash = "a".repeat(HASH_LEN);
        assert!(Evidence { symbol: "s".into(), hash: hash.clone() }.validated().is_ok());
        assert!(Evidence { symbol: " ".into(), hash }.validated().is_err());
        assert!(Evidence { symbol: "s".into(), hash: "A".repeat(HASH_LEN) }.validated().is_err());
        assert!(Evidence { symbol: "s".into(), hash: "a".repeat(HASH_LEN - 1) }.validated().is_err());
    }
}
