//! **S-475's blocking measurement gate** — does [S-411]'s addressed-pair half
//! hold once the configuration corpus reads YAML sequence items?
//!
//! [S-411] measured the addressed-pair half one pair below its floor and named,
//! before its run, a blind spot in its own reader that was larger than the
//! margin: the shipped `parse_yaml` binds no scalar under a YAML sequence, and
//! the estate's gateway upstream tables (`proxy.upstreams[].host`) are
//! sequences. [CR-153] re-proposes the gate with **one** change — the corpus
//! reads each sequence item under an item-scoped key — against the **same**
//! metric and the **same unrevised** floor. This module is that re-run.
//!
//! The floor ([`ADDRESSED_PAIR_FLOOR`]), the decisive population and the reading
//! rules are declared in [`sequence_addressed_pairs_floor.txt`], committed
//! before this module existed; [`the_floor_and_the_population_are_the_declared_ones`]
//! parses both out of it, so neither can be edited to clear a run without the
//! declaration being edited too.
//!
//! # Everything but the reader is S-411's, called
//!
//! The walk is [S-411]'s [`walk_overlays_with`] — the same traversal, admission,
//! documentation guard and overlay attribution [`walk_overlays`] runs, with the
//! flattener as its one parameter. The admission rule is [S-411]'s
//! [`collect_values`], the classifier its [`label_state`] and [`classify_pair`],
//! the runnable set and the path-only subtraction its [`Judgement`]'s own. The
//! baseline those readings are compared with is [S-411]'s [`judgement`], so
//! "pairs gained over S-411" is read against the object S-411 itself reports,
//! not a reconstruction of it.
//!
//! What a sequence reading adds to a source decomposes exactly: the admission
//! rule judges a key by its own value and, for a bare host, by a sibling `port`
//! under the **same** parent — and an item-scoped key's parent is its item — so
//! the targets of *shipped reading ∪ item keys* are the shipped reading's
//! targets plus the item keys' own. [S-411]'s judged targets are therefore
//! reused as they are, and only the item keys are judged here.
//!
//! # The reader is harness-local, and borrows every scalar rule it has
//!
//! [`read_items`] is the only new rule, and it is a **rewrite**, not a parser: it
//! turns each sequence item into a mapping under a sentinel key
//! (`zzlogosseqitem<N>zz:`) directly beneath the sequence's own key, runs the
//! **shipped** `parse_yaml` over the rewritten text, and renames each sentinel
//! segment to the item's `[index]`. Comments, quoting, escape refusal, block and
//! flow skips, anchors, nested mappings inside an item and relaxed-binding
//! canonicalisation are therefore the shipped parser's own, not a second
//! spelling of them. A nested sequence inside an item is still skipped by the
//! shipped parser, which is [CR-153] §3.2 A rule 3.
//!
//! The rewrite is checked against the shipped parser on **every** source, two
//! ways. First ([`divergent_keys`]): every key it does not scope to an item must
//! read exactly as the shipped parser reads it. A source where that fails is one
//! whose line structure the rewrite cannot scope faithfully, and it is
//! **refused whole** — it adds nothing, which is the under-read direction the
//! declaration allows. The estate's case is a Helm template, where a column-0
//! `{{- range }}` / `{{- end }}` directive cuts an item body in two: the shipped
//! parser then hangs the item's remaining lines under the enclosing key (its own
//! `spec.template.spec.containers.image` is the `routes:` fabrication, still
//! live on templates), and the rewrite hangs them under the item instead. What
//! the refusal forgoes is measured and printed, never assumed. Second
//! ([`collapsed_keys`]): every key a faithful source adds must carry an index.
//! Any one that does not **fails the run** — the `routes:` incident is exactly
//! an index-free key read out of a list, and refusing it would hide a defect of
//! the reader rather than decline a source.
//!
//! # Three readings, one decisive
//!
//! [`Reading::Directory`] is [S-411]'s 84-directory population, printed for
//! comparability. [`Reading::Manifest`] counts only pairs whose two ends are
//! members of the workspace manifest. [`Reading::Strict`] also leaves out the
//! test-harness consumer, and it is the one the verdict is read off: [CR-153]
//! §3.2 B4 records a HOLDS that depends on the directory population or on
//! `e2e-tests` as FALSIFIED on the decisive reading.
//!
//! [CR-153]: ../../../docs/requests/CR-153-reread-addressed-pairs-through-yaml-sequences.md
//! [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
//! [`sequence_addressed_pairs_floor.txt`]: ./sequence_addressed_pairs_floor.txt
//! [`walk_overlays`]: super::config_declared_coupling::walk_overlays

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::federation::discover;

use super::config_declared_coupling::{
    classify_pair, collect_values, judgement, label_state, walk_overlays_with, JudgedTarget,
    Judgement, PairOutcome, Provenance, Scalar, SequenceHost, SourceSet, Target, WalkCost,
};
use super::configuration_agreement::parse_yaml;
use super::identity::{self, Corpus};

/// **The floor and the population, as declared before the run.**
///
/// `include_str!` rather than a path reference, following
/// [`super::config_declared_coupling::DECLARED_FLOOR`]: a file the build embeds
/// cannot be deleted or renamed without breaking compilation.
pub const DECLARED_FLOOR: &str = include_str!("sequence_addressed_pairs_floor.txt");

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("sequence_addressed_pairs_finding.txt");

/// The addressed-pair floor, parsed out of [`DECLARED_FLOOR`] by
/// [`the_floor_and_the_population_are_the_declared_ones`]. [CR-131]'s figure,
/// unrevised.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
pub const ADDRESSED_PAIR_FLOOR: usize = 12;

/// The one consumer the metric admits that is not a production deployment: the
/// end-to-end suite whose `final-tests/` docker-compose [S-411] counted and
/// flagged. Named in the declaration; [`Reading::Strict`] does not count it.
///
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
pub const TEST_HARNESS_CONSUMER: &str = "e2e-tests";

/// The `host:`-shaped lines [S-411] found inside a sequence item in an admitted
/// deploy file — the textual ceiling its finding records, and the population
/// this reader's coverage is reconciled against.
///
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
pub const S411_CEILING_LINES: usize = 84;

// ── The item-scoped sequence reader ─────────────────────────────────────────

/// The sentinel an item is rewritten under: prefix, item id, suffix. Lower-case
/// with no `-` or `_`, so the shipped `canonical_key` leaves it intact.
const ITEM_SENTINEL: (&str, &str) = ("zzlogosseqitem", "zz");

/// A key appended beneath one line to ask the shipped parser what that line is.
const PROBE: &str = "zzlogosseqprobe";

/// Why a sequence item binds nothing. Every item the reader meets lands either
/// here or in a bound count, so the reader states its own coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    /// The line before the first item is not the key whose value the sequence
    /// is — a document-root sequence, or one under a key the shipped parser
    /// does not read. There is no key to scope the item under.
    NoParentKey,
    /// `- - a`: a sequence directly inside a sequence (rule 3).
    NestedSequence,
    /// `- [a, b]` (rule 3).
    FlowSequence,
    /// `- {a: b}`.
    FlowMapping,
    /// `- |` / `- >` (rule 3).
    BlockScalar,
    /// `- &anchor` / `- *alias`: a reference, not a value.
    AnchorOrAlias,
    /// A plain scalar item continued on further lines.
    MultiLineScalar,
    /// `-x` with no space, or a mapping item whose key holds a `:` the shipped
    /// key split would cut in the wrong place.
    Unreadable,
    /// The shipped parser reads no scalar out of the item: an empty item, a
    /// header-only mapping, an escaped quote it refuses.
    NothingRead,
    /// The source already contains the sentinel text; the whole source is left
    /// unread rather than risk a real key being renamed.
    SentinelInSource,
    /// Every item of a source whose rewrite did not reproduce the shipped
    /// reading key for key — see [`divergent_keys`]. Counted per item met.
    SourceRefused,
}

impl Shape {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoParentKey => "no parent key (document root / unread key)",
            Self::NestedSequence => "nested sequence (`- - a`)",
            Self::FlowSequence => "flow sequence (`- [a, b]`)",
            Self::FlowMapping => "flow mapping (`- {a: b}`)",
            Self::BlockScalar => "block scalar (`- |`)",
            Self::AnchorOrAlias => "anchor or alias (`- &a` / `- *a`)",
            Self::MultiLineScalar => "multi-line plain scalar",
            Self::Unreadable => "unreadable item (`-x`, key holding `:`)",
            Self::NothingRead => "nothing the shipped parser reads",
            Self::SentinelInSource => "source contains the sentinel",
            Self::SourceRefused => "source refused: rewrite diverges from shipped",
        }
    }
}

/// The reader's own account of what it did with every sequence item it met.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Sequences with a parent key, i.e. whose items could be scoped.
    pub sequences: usize,
    /// Items whose mapping yielded at least one item-scoped scalar.
    pub mapping_items_bound: usize,
    /// `- value` items bound under `key[i]`.
    pub scalar_items_bound: usize,
    /// Items that bound nothing, by why.
    pub skipped: BTreeMap<Shape, usize>,
    /// Distinct item-scoped keys produced.
    pub keys_bound: usize,
    /// `-`-led lines inside a bound item's body — nested sequences, left to
    /// the shipped parser's skip (rule 3).
    pub nested_lines_in_items: usize,
    /// The first item met in each skip bucket, so a bucket is evidence rather
    /// than a bare count.
    pub examples: BTreeMap<Shape, String>,
}

impl Coverage {
    pub fn items_bound(&self) -> usize {
        self.mapping_items_bound + self.scalar_items_bound
    }

    pub fn items_skipped(&self) -> usize {
        self.skipped.values().sum()
    }

    pub fn absorb(&mut self, other: &Coverage) {
        self.sequences += other.sequences;
        self.mapping_items_bound += other.mapping_items_bound;
        self.scalar_items_bound += other.scalar_items_bound;
        self.keys_bound += other.keys_bound;
        self.nested_lines_in_items += other.nested_lines_in_items;
        for (shape, n) in &other.skipped {
            *self.skipped.entry(*shape).or_default() += n;
        }
        for (shape, example) in &other.examples {
            self.examples
                .entry(*shape)
                .or_insert_with(|| example.clone());
        }
    }

    fn skip(&mut self, shape: Shape, example: &str) {
        *self.skipped.entry(shape).or_default() += 1;
        self.examples
            .entry(shape)
            .or_insert_with(|| example.trim().to_string());
    }
}

/// What [`read_items`] makes of one source.
#[derive(Debug, Default, Clone)]
pub struct ItemReading {
    /// Item-scoped keys ONLY. The shipped reading of the same source is
    /// `parse_yaml(text)`; this is what the sequence reading adds to it.
    pub values: BTreeMap<String, BTreeSet<String>>,
    pub coverage: Coverage,
    /// Every collapsed key, each described — see [`collapsed_keys`]. Non-empty
    /// fails the run.
    pub collapsed: Vec<String>,
    /// Set when the source was refused: the divergences that refused it, and
    /// the item-scoped keys it would otherwise have added — kept so the gate
    /// can measure what the refusal forgoes.
    pub refused: Option<Refusal>,
}

/// Why one source was refused, and what refusing it cost.
#[derive(Debug, Default, Clone)]
pub struct Refusal {
    pub divergences: Vec<String>,
    pub forgone: BTreeMap<String, BTreeSet<String>>,
}

/// How an item is planned before the shipped parser reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Mapping,
    Scalar,
}

/// One item the rewrite emitted, keyed by the id its sentinel carries.
struct Planned {
    kind: ItemKind,
    index: usize,
    first: String,
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The shipped parser's own test for "this line opens a sequence item".
fn is_item_line(trimmed: &str) -> bool {
    trimmed.starts_with('-') && trimmed != "---" && !trimmed.starts_with("--- ")
}

/// What one mapping line is, asked of the shipped parser by appending a probe
/// key beneath it: a header (the probe lands under it), a block-scalar opener
/// (the probe is swallowed as block text), or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Header,
    BlockOpener,
    Other,
}

fn line_kind(line: &str) -> LineKind {
    let read = parse_yaml(&format!("{}\n {PROBE}: x", line.trim()));
    if read.is_empty() {
        return LineKind::BlockOpener;
    }
    let under_header = read.len() == 1
        && read
            .keys()
            .next()
            .is_some_and(|k| k.len() > PROBE.len() + 1 && k.ends_with(&format!(".{PROBE}")));
    if under_header {
        LineKind::Header
    } else {
        LineKind::Other
    }
}

/// What an item's first line is under YAML's rule for a mapping key — a `:`
/// followed by whitespace or the end of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemHead<'a> {
    /// A mapping entry with this key.
    Mapping(&'a str),
    /// A plain or quoted scalar.
    Scalar,
    /// A mapping entry whose key is not one token (`- a b: c`). The shipped
    /// key split refuses such a key at mapping level; read as a scalar
    /// instead, the whole line would become a wrong value.
    Unreadable,
}

/// The [`ItemHead`] of a mapping item's first line.
///
/// Not the shipped key split, deliberately: that one cuts at the FIRST `:`, so
/// `- http://mailbox-api:8080` would read as a mapping `{http: //mailbox-api:8080}`
/// and fabricate a `hosts[0].http` key out of a URL. At mapping level the
/// shipped parser never meets that shape; inside a sequence it is the commonest
/// scalar there is.
fn mapping_item_key(body: &str) -> ItemHead<'_> {
    let is_value_colon = |after: &str| after.is_empty() || after.starts_with([' ', '\t']);
    for quote in ['"', '\''] {
        if let Some(rest) = body.strip_prefix(quote) {
            let Some(end) = rest.find(quote) else { return ItemHead::Scalar };
            // `"u\": http://x"` is ONE string whose first quote is escaped.
            // Ending the key at that quote would read `{u\: http://x"}` — an
            // invented key and an admitted URL. Left a scalar instead, the
            // shipped parser's escape refusal declines it, as it declines an
            // escaped value at mapping level.
            if quote == '"' && rest[..end].contains('\\') {
                return ItemHead::Scalar;
            }
            let after = rest[end + 1..].trim_start_matches([' ', '\t']);
            return match after.strip_prefix(':') {
                Some(a) if is_value_colon(a) => ItemHead::Mapping(&rest[..end]),
                _ => ItemHead::Scalar,
            };
        }
    }
    let Some(colon) = body
        .char_indices()
        .find(|(i, c)| *c == ':' && is_value_colon(&body[i + 1..]))
        .map(|(i, _)| i)
    else {
        return ItemHead::Scalar;
    };
    let key = &body[..colon];
    if key.is_empty() || key.contains(char::is_whitespace) {
        ItemHead::Unreadable
    } else {
        ItemHead::Mapping(key)
    }
}

/// Decide what one item is, from the text after its dash and its continuation
/// lines. `Err` is the shape that leaves it unbound.
fn plan_item(after_dash: &str, continuation: &[&str]) -> Result<ItemKind, Shape> {
    let content: Vec<&str> = continuation
        .iter()
        .map(|l| l.trim())
        .filter(|t| !t.is_empty() && !t.starts_with('#'))
        .collect();
    if !after_dash.is_empty() && !after_dash.starts_with([' ', '\t']) {
        return Err(Shape::Unreadable);
    }
    let body = after_dash.trim();
    if body.is_empty() || body.starts_with('#') {
        return match content.first() {
            None => Err(Shape::NothingRead),
            Some(first) if is_item_line(first) => Err(Shape::NestedSequence),
            Some(_) => Ok(ItemKind::Mapping),
        };
    }
    match body.chars().next() {
        Some('-') if body == "-" || body.starts_with("- ") || body.starts_with("-\t") => {
            return Err(Shape::NestedSequence)
        }
        Some('[') => return Err(Shape::FlowSequence),
        Some('{') => return Err(Shape::FlowMapping),
        Some('|' | '>') => return Err(Shape::BlockScalar),
        Some('&' | '*') => return Err(Shape::AnchorOrAlias),
        _ => {}
    }
    match mapping_item_key(body) {
        ItemHead::Mapping(key) if key.contains(':') => Err(Shape::Unreadable),
        ItemHead::Mapping(_) => Ok(ItemKind::Mapping),
        ItemHead::Unreadable => Err(Shape::Unreadable),
        ItemHead::Scalar if !content.is_empty() => Err(Shape::MultiLineScalar),
        ItemHead::Scalar => Ok(ItemKind::Scalar),
    }
}

/// **The item-scoped sequence reading** ([CR-153] §3.2 A) of one YAML source:
/// the scalars of each sequence item, bound under `key[i]` / `key[i].leaf`.
///
/// Returns the item-scoped keys only; the full reading of the source is these
/// plus `parse_yaml(text)`, unchanged. See this module's docs for why it is a
/// rewrite through the shipped parser rather than a parser of its own.
///
/// [CR-153]: ../../../docs/requests/CR-153-reread-addressed-pairs-through-yaml-sequences.md
pub fn read_items(text: &str) -> ItemReading {
    let mut out = ItemReading::default();
    if text.contains(ITEM_SENTINEL.0) || text.contains(PROBE) {
        out.coverage.skip(Shape::SentinelInSource, "");
        return out;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut rewritten: Vec<String> = Vec::with_capacity(lines.len());
    let mut planned: Vec<Planned> = Vec::new();
    let mut block_at: Option<usize> = None;
    let mut previous: Option<&str> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = indent_of(line);
        let trimmed = line.trim();
        if let Some(block) = block_at {
            if trimmed.is_empty() || indent > block {
                rewritten.push(line.to_string());
                i += 1;
                continue;
            }
            block_at = None;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            rewritten.push(line.to_string());
            i += 1;
            continue;
        }
        if !is_item_line(trimmed) {
            if trimmed == "---" || trimmed.starts_with("--- ") {
                previous = None;
            } else {
                if (trimmed.contains('|') || trimmed.contains('>'))
                    && line_kind(line) == LineKind::BlockOpener
                {
                    block_at = Some(indent);
                }
                previous = Some(line);
            }
            rewritten.push(line.to_string());
            i += 1;
            continue;
        }
        // A sequence starts here, at `indent`.
        let parented =
            previous.is_some_and(|p| indent_of(p) <= indent && line_kind(p) == LineKind::Header);
        if parented {
            out.coverage.sequences += 1;
        }
        let mut index = 0usize;
        loop {
            let start = i;
            i += 1;
            while i < lines.len() && {
                let t = lines[i].trim();
                t.is_empty() || t.starts_with('#') || indent_of(lines[i]) > indent
            } {
                i += 1;
            }
            let item = &lines[start..i];
            if parented {
                rewrite_item(
                    item,
                    indent,
                    index,
                    &mut planned,
                    &mut rewritten,
                    &mut out.coverage,
                );
            } else {
                out.coverage.skip(Shape::NoParentKey, item[0]);
                rewritten.extend(item.iter().map(|l| (*l).to_string()));
            }
            index += 1;
            let next_is_item =
                i < lines.len() && indent_of(lines[i]) == indent && is_item_line(lines[i].trim());
            if !next_is_item {
                break;
            }
        }
        previous = None;
    }
    finish(text, &rewritten.join("\n"), &planned, out)
}

/// Emit one item of a parented sequence in its rewritten form, or count why not.
fn rewrite_item(
    item: &[&str],
    indent: usize,
    index: usize,
    planned: &mut Vec<Planned>,
    rewritten: &mut Vec<String>,
    coverage: &mut Coverage,
) {
    let first = item[0];
    let after_dash = &first[indent + 1..];
    let kind = match plan_item(after_dash, &item[1..]) {
        Ok(kind) => kind,
        Err(shape) => {
            // Dropped, not emitted: the shipped parser skipped these lines too,
            // and left in they would re-arm its sequence skip over the
            // rewritten items that follow.
            coverage.skip(shape, first);
            return;
        }
    };
    let id = planned.len();
    planned.push(Planned {
        kind,
        index,
        first: first.to_string(),
    });
    let header = format!(
        "{}{}{id}{}:",
        " ".repeat(indent + 1),
        ITEM_SENTINEL.0,
        ITEM_SENTINEL.1
    );
    match kind {
        ItemKind::Scalar => rewritten.push(format!("{header} {}", after_dash.trim())),
        ItemKind::Mapping => {
            rewritten.push(header);
            // The dash becomes a space and every line shifts right by two, so
            // the item's own alignment is kept and all of it sits below the
            // sentinel at `indent + 1`.
            rewritten.push(format!("  {} {}", &first[..indent], after_dash));
            for line in &item[1..] {
                if is_item_line(line.trim()) {
                    coverage.nested_lines_in_items += 1;
                }
                rewritten.push(format!("  {line}"));
            }
        }
    }
}

/// Read the rewritten text with the shipped parser, rename the sentinels, and
/// check the result against the shipped reading of the original.
fn finish(
    original: &str,
    rewritten: &str,
    planned: &[Planned],
    mut out: ItemReading,
) -> ItemReading {
    let shipped = parse_yaml(original);
    let read = parse_yaml(rewritten);
    let mut plain: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut bound_ids: BTreeSet<usize> = BTreeSet::new();
    for (key, values) in read {
        match scope_key(&key, planned) {
            Some((scoped, id)) => {
                bound_ids.insert(id);
                out.values.entry(scoped).or_default().extend(values);
            }
            None => {
                plain.insert(key, values);
            }
        }
    }
    for (id, item) in planned.iter().enumerate() {
        match (bound_ids.contains(&id), item.kind) {
            (true, ItemKind::Mapping) => out.coverage.mapping_items_bound += 1,
            (true, ItemKind::Scalar) => out.coverage.scalar_items_bound += 1,
            (false, _) => out.coverage.skip(Shape::NothingRead, &item.first),
        }
    }
    let divergences = divergent_keys(&shipped, &plain);
    if !divergences.is_empty() {
        let met = out.coverage.items_bound() + out.coverage.items_skipped();
        let mut refused = Coverage::default();
        refused.skipped.insert(Shape::SourceRefused, met);
        refused
            .examples
            .insert(Shape::SourceRefused, divergences[0].clone());
        return ItemReading {
            values: BTreeMap::new(),
            coverage: refused,
            collapsed: Vec::new(),
            refused: Some(Refusal {
                divergences,
                forgone: out.values,
            }),
        };
    }
    out.coverage.keys_bound = out.values.len();
    out.collapsed = collapsed_keys(&out.values);
    out
}

/// Rename the one sentinel segment of a rewritten key to its item's index:
/// `proxy.upstreams.zzlogosseqitem3zz.host` → `proxy.upstreams[1].host`.
///
/// `None` for a key carrying no sentinel. A key whose sentinel is its FIRST
/// segment, or that carries two, is returned with the sentinel intact, so the
/// collapsed-key check reports it rather than this function hiding it.
fn scope_key(key: &str, planned: &[Planned]) -> Option<(String, usize)> {
    let segments: Vec<&str> = key.split('.').collect();
    let ids: Vec<(usize, usize)> = segments
        .iter()
        .enumerate()
        .filter_map(|(at, seg)| {
            let id = seg
                .strip_prefix(ITEM_SENTINEL.0)?
                .strip_suffix(ITEM_SENTINEL.1)?;
            Some((at, id.parse::<usize>().ok()?))
        })
        .collect();
    let &[(at, id)] = ids.as_slice() else {
        return (!ids.is_empty()).then(|| (key.to_string(), ids[0].1));
    };
    let Some(item) = planned.get(id).filter(|_| at > 0) else {
        return Some((key.to_string(), id));
    };
    let mut scoped = segments[..at].join(".");
    scoped.push_str(&format!("[{}]", item.index));
    for seg in &segments[at + 1..] {
        scoped.push('.');
        scoped.push_str(seg);
    }
    Some((scoped, id))
}

/// **The faithfulness check** — where the rewritten text, outside every item,
/// does not read exactly as the shipped parser reads the original.
///
/// - a key the shipped parser reads must read identically (`plain` is
///   everything the rewritten text yields outside an item);
/// - a key outside an item that the shipped parser does NOT read means an
///   item's lines registered under its enclosing mapping.
///
/// Either one means the rewrite could not scope this source's items as the
/// shipped parser bounds them, and [`read_items`] refuses the source whole.
pub fn divergent_keys(
    shipped: &BTreeMap<String, BTreeSet<String>>,
    plain: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (key, values) in shipped {
        if plain.get(key) != Some(values) {
            out.push(format!(
                "shipped key `{key}` reads {:?} beside the rewrite",
                plain.get(key)
            ));
        }
    }
    for key in plain.keys().filter(|k| !shipped.contains_key(*k)) {
        out.push(format!(
            "`{key}` is produced outside any item and the shipped parser never read it"
        ));
    }
    out
}

/// **The collapsed-key check** — every item key a faithful source adds must be
/// item-scoped: `…[N]` on a named key, no sentinel left, no index-free form.
/// Any one that is not FAILS THE RUN; it is never a reason to refuse a source,
/// because it is a defect of the reader, not a property of the source.
///
/// A pure function, so the check itself is pinned by fixtures that feed it each
/// violation independently of the reader it guards.
pub fn collapsed_keys(items: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    items
        .keys()
        .filter(|k| !is_item_scoped(k))
        .map(|k| format!("`{k}` is admitted from a sequence without an item index"))
        .collect()
}

/// A key names an item: some segment is `name[digits]` with a non-empty name,
/// and no sentinel text survives.
pub fn is_item_scoped(key: &str) -> bool {
    !key.contains(ITEM_SENTINEL.0)
        && key.split('.').any(|seg| {
            seg.strip_suffix(']')
                .and_then(|s| s.rsplit_once('['))
                .is_some_and(|(name, idx)| {
                    !name.is_empty() && !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit())
                })
        })
}

// ── The re-measurement ──────────────────────────────────────────────────────

/// The three populations the pair set is counted over — see the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    /// [S-411]'s 84 `.git` directories. Never decisive.
    ///
    /// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
    Directory,
    /// Both ends manifest members, test-harness consumer counted.
    Manifest,
    /// [`Reading::Manifest`] without the test-harness consumer — **the gate**.
    Strict,
}

impl Reading {
    pub const ALL: [Self; 3] = [Self::Directory, Self::Manifest, Self::Strict];

    pub fn label(self) -> &'static str {
        match self {
            Self::Directory => "DIRECTORY (84 .git dirs, S-411's population)",
            Self::Manifest => "MANIFEST (83 members, e2e-tests counted)",
            Self::Strict => "STRICT (83 members, e2e-tests not counted) — THE GATE",
        }
    }

    /// Whether this reading counts the ordered pair `(a, b)` — the one
    /// definition of each population.
    pub fn counts(self, a: &str, b: &str, manifest: &BTreeSet<String>) -> bool {
        match self {
            Self::Directory => true,
            Self::Manifest => manifest.contains(a) && manifest.contains(b),
            Self::Strict => {
                a != TEST_HARNESS_CONSUMER && manifest.contains(a) && manifest.contains(b)
            }
        }
    }

    /// Why a pair this reading does not count was left out — for the "lost"
    /// enumeration.
    pub fn why_not(self, a: &str, b: &str, manifest: &BTreeSet<String>) -> &'static str {
        if !manifest.contains(a) {
            "consumer is not a manifest member"
        } else if !manifest.contains(b) {
            "provider is not a manifest member"
        } else if self == Self::Strict && a == TEST_HARNESS_CONSUMER {
            "test-harness consumer"
        } else {
            "not addressed under this reading"
        }
    }
}

/// One `host`/`hostname` scalar the reader bound inside a deploy file's item.
#[derive(Debug, Clone)]
pub struct BoundHost {
    pub file: String,
    pub key: String,
    pub value: String,
}

/// Everything the re-run measures.
pub struct Remeasure {
    /// [S-411]'s own judgement — the baseline, and the source of its runnable
    /// set, path-only pairs and shipped-reader targets.
    ///
    /// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
    pub s411: &'static Judgement,
    /// The manifest members, as the shipped `discover` reads them.
    pub manifest: BTreeSet<String>,
    /// Targets the item-scoped keys add, judged by S-411's classifier.
    pub item_targets: Vec<JudgedTarget>,
    pub deploy_coverage: Coverage,
    pub application_coverage: Coverage,
    /// `file: description` for every collapsed key, on either source set.
    pub collapsed: Vec<String>,
    /// Sources refused because the rewrite diverged, with the refusal.
    pub refused: Vec<(String, Refusal)>,
    /// Targets the refused sources' item keys would have admitted, judged —
    /// what the refusal costs, measured.
    pub forgone_targets: Vec<JudgedTarget>,
    pub bound_hosts: Vec<BoundHost>,
    /// This run's own ceiling, from the same walk — reconciled with S-411's.
    pub ceiling: Vec<SequenceHost>,
    /// This run's own walk — the gate's denominator.
    pub cost: WalkCost,
    /// Application YAML sources the reader re-read.
    pub application_sources: usize,
    /// Pairs a manifest-scoped identity registry would add on STRICT, with the
    /// label that stops colliding. A sensitivity, never counted.
    pub sensitivity: BTreeSet<(String, String, String)>,
}

impl Remeasure {
    /// S-411's judged targets and the item-scoped ones — the whole population
    /// the sequence-reading corpus yields.
    pub fn targets(&self) -> impl Iterator<Item = &JudgedTarget> {
        self.s411.targets.iter().chain(&self.item_targets)
    }

    /// Distinct addressed ordered pairs under one reading — the grain the floor
    /// is read at.
    pub fn pairs(&self, reading: Reading) -> BTreeSet<(&str, &str)> {
        addressed_pairs(self.targets(), reading, &self.manifest)
    }

    /// The pairs [S-411] recorded — its own object, not a reconstruction.
    ///
    /// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
    pub fn s411_pairs(&self) -> BTreeSet<(&str, &str)> {
        self.s411.pairs(PairOutcome::Addressed)
    }

    /// The targets proving one pair, for the gained enumeration.
    pub fn evidence(&self, a: &str, b: &str) -> Vec<&Target> {
        self.targets()
            .filter(|t| {
                t.outcome == PairOutcome::Addressed
                    && t.target.member == a
                    && t.provider.as_deref() == Some(b)
            })
            .map(|t| &t.target)
            .collect()
    }
}

/// The addressed-pair set of a target population under one reading. A free
/// function so the population rules are pinned by fixtures without an estate.
pub fn addressed_pairs<'a>(
    targets: impl Iterator<Item = &'a JudgedTarget>,
    reading: Reading,
    manifest: &BTreeSet<String>,
) -> BTreeSet<(&'a str, &'a str)> {
    targets
        .filter(|t| t.outcome == PairOutcome::Addressed)
        .filter_map(|t| Some((t.target.member.as_str(), t.provider.as_deref()?)))
        .filter(|(a, b)| reading.counts(a, b, manifest))
        .collect()
}

/// **The verdict, in one place.** HOLDS iff the decisive reading reaches the
/// floor; the other two readings never move it.
pub fn verdict(strict: usize) -> &'static str {
    if strict >= ADDRESSED_PAIR_FLOOR {
        "HOLDS"
    } else {
        "FALSIFIED"
    }
}

/// The headline line, or VOID where no estate was measured ([NFR-CC-04]): zero
/// pairs is not a finding, and a run that saw nothing must not print one.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub fn headline(strict: Option<usize>) -> String {
    match strict {
        None => "VOID: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
                 S-475 sequence-reading addressed-pair gate. A run that sees no estate reports \
                 VOID, never zero (see sequence_addressed_pairs_floor.txt)."
            .to_string(),
        Some(n) => format!(
            "VERDICT: addressed member pairs on the decisive STRICT reading {n} against a floor \
             of {ADDRESSED_PAIR_FLOOR} declared before the run  =>  {}",
            verdict(n)
        ),
    }
}

/// The manifest members listed in the declaration, between its two markers.
pub fn declared_members() -> BTreeSet<String> {
    let mut inside = false;
    let mut out = BTreeSet::new();
    for line in DECLARED_FLOOR.lines() {
        if line.starts_with("MANIFEST MEMBERS — BEGIN") {
            inside = true;
        } else if line.starts_with("MANIFEST MEMBERS — END") {
            inside = false;
        } else if inside && !line.trim().is_empty() {
            out.insert(line.trim().to_string());
        }
    }
    out
}

/// Run the re-measurement.
pub fn remeasure(root: &Path) -> Remeasure {
    let s411 = judgement(root);
    let corpus = &identity::findings(root).corpus;
    let manifest: BTreeSet<String> = discover(root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| panic!("{} holds no logos.workspace.toml", root.display()))
        .members
        .into_iter()
        .map(|m| m.name)
        .collect();

    let mut targets: Vec<Target> = Vec::new();
    let mut scalars: Vec<Scalar> = Vec::new();
    let mut deploy_coverage = Coverage::default();
    let mut collapsed = Vec::new();
    let mut refused: Vec<(String, Refusal)> = Vec::new();
    let mut bound_hosts = Vec::new();

    let overlays = walk_overlays_with(root, &s411.members, &mut |rel, text| {
        if rel.ends_with(".properties") {
            return BTreeMap::new();
        }
        let reading = read_items(text);
        deploy_coverage.absorb(&reading.coverage);
        collapsed.extend(reading.collapsed.iter().map(|c| format!("{rel}: {c}")));
        if let Some(refusal) = &reading.refused {
            refused.push((rel.to_string(), refusal.clone()));
        }
        for (key, values) in &reading.values {
            let leaf = key.rsplit('.').next().unwrap_or(key);
            if leaf == "host" || leaf == "hostname" {
                for value in values {
                    bound_hosts.push(BoundHost {
                        file: rel.to_string(),
                        key: key.clone(),
                        value: value.clone(),
                    });
                }
            }
        }
        reading.values
    });
    targets.extend(overlays.values);

    let mut application_coverage = Coverage::default();
    let mut application_sources = 0usize;
    for source in &crate::measurement(root).config.sources {
        let member = source.path.split('/').next().unwrap_or("");
        if !s411.members.contains(member) || source.path.ends_with(".properties") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&source.path)) else {
            continue;
        };
        application_sources += 1;
        let reading = read_items(&text);
        application_coverage.absorb(&reading.coverage);
        collapsed.extend(
            reading
                .collapsed
                .iter()
                .map(|c| format!("{}: {c}", source.path)),
        );
        if let Some(refusal) = &reading.refused {
            refused.push((source.path.clone(), refusal.clone()));
        }
        let overlay = identity::application_overlay(source.profile.as_deref());
        collect_values(
            &Provenance {
                member,
                file: &source.path,
                overlay: &overlay,
                source: SourceSet::Application,
            },
            &reading.values,
            &mut scalars,
            &mut targets,
        );
    }

    let judge = |target: Target| {
        let state = label_state(corpus, &target.label);
        let (outcome, provider) =
            classify_pair(&target.member, state, &s411.runnable, &s411.path_only);
        JudgedTarget {
            target,
            outcome,
            provider,
        }
    };
    let item_targets: Vec<JudgedTarget> = targets.into_iter().map(judge).collect();

    // What the refusals cost, through the same admission rule and classifier.
    // Every refused source is tagged as a deploy overlay or application config
    // by where it was read; the overlay is its file's own.
    let mut forgone: Vec<Target> = Vec::new();
    for (file, refusal) in &refused {
        let member = file.split('/').next().unwrap_or("");
        let app = crate::measurement(root)
            .config
            .sources
            .iter()
            .find(|s| &s.path == file);
        let (overlay, source) = match app {
            Some(s) => (
                identity::application_overlay(s.profile.as_deref()),
                SourceSet::Application,
            ),
            None => (identity::overlay_of(file, member), SourceSet::Deploy),
        };
        collect_values(
            &Provenance {
                member,
                file,
                overlay: &overlay,
                source,
            },
            &refusal.forgone,
            &mut Vec::new(),
            &mut forgone,
        );
    }
    let forgone_targets: Vec<JudgedTarget> = forgone.into_iter().map(judge).collect();

    let mut out = Remeasure {
        s411,
        manifest,
        item_targets,
        deploy_coverage,
        application_coverage,
        collapsed,
        refused,
        forgone_targets,
        bound_hosts,
        ceiling: overlays.sequence_host_ceiling,
        cost: overlays.cost,
        application_sources,
        sensitivity: BTreeSet::new(),
    };
    out.sensitivity = manifest_registry_sensitivity(&out, corpus);
    out
}

/// Pairs STRICT would gain if the identity corpus held only manifest members'
/// claims — a label that collides only because a non-member claims it too.
///
/// A sensitivity, printed and never counted: it changes the classifier, which
/// the declaration forbids, and its pairs never went through a path-only
/// subtraction taken without the non-member.
pub fn manifest_registry_sensitivity(
    r: &Remeasure,
    corpus: &Corpus,
) -> BTreeSet<(String, String, String)> {
    let scoped = Corpus {
        members: r.manifest.clone(),
        claims: corpus
            .claims
            .iter()
            .filter(|c| r.manifest.contains(&c.member))
            .cloned()
            .collect(),
        ..Corpus::default()
    };
    let strict = r.pairs(Reading::Strict);
    r.targets()
        .filter(|t| t.outcome == PairOutcome::SameTierCollision)
        .filter_map(|t| {
            let state = label_state(&scoped, &t.target.label);
            let (outcome, provider) =
                classify_pair(&t.target.member, state, &r.s411.runnable, &r.s411.path_only);
            let provider = provider.filter(|_| outcome == PairOutcome::Addressed)?;
            let a = t.target.member.as_str();
            (Reading::Strict.counts(a, &provider, &r.manifest)
                && !strict.contains(&(a, provider.as_str())))
            .then(|| (a.to_string(), provider, t.target.label.clone()))
        })
        .collect()
}

/// Unbound ceiling lines whose value is a Helm template expression — a value a
/// deploy tool substitutes, which neither the URL parser nor the bare-host rule
/// can ever read as an identity, whatever reader sits in front of them.
pub fn templated(unmatched: &[&SequenceHost]) -> usize {
    unmatched.iter().filter(|l| l.value.contains("{{")).count()
}

/// S-411's ceiling lines, each matched against a `host`/`hostname` scalar the
/// reader bound in the same file with the same value (each bound scalar
/// consumed once). Returns `(matched, the unmatched ceiling lines, the bound
/// hosts no ceiling line accounts for)`.
pub fn reconcile_ceiling<'a>(
    ceiling: &'a [SequenceHost],
    bound: &'a [BoundHost],
) -> (usize, Vec<&'a SequenceHost>, Vec<&'a BoundHost>) {
    let mut pool: Vec<Option<&BoundHost>> = bound.iter().map(Some).collect();
    let mut matched = 0;
    let mut unmatched = Vec::new();
    for line in ceiling {
        let hit = pool
            .iter_mut()
            .find(|slot| slot.is_some_and(|b| b.file == line.file && b.value == line.value));
        match hit {
            Some(slot) => {
                *slot = None;
                matched += 1;
            }
            None => unmatched.push(line),
        }
    }
    let extra = pool.into_iter().flatten().collect();
    (matched, unmatched, extra)
}

// ── The report ──────────────────────────────────────────────────────────────

fn report(r: &Remeasure) {
    println!("\n=== S-475 · the addressed-pair gate through a YAML-sequence-reading corpus ===");
    report_readings(r);
    for reading in Reading::ALL {
        report_gained_and_lost(r, reading);
    }
    report_sensitivity(r);
    report_coverage(r);
    report_walk(r);
}

fn report_readings(r: &Remeasure) {
    println!("\n  THE THREE READINGS (floor {ADDRESSED_PAIR_FLOOR}, declared before the run)\n");
    println!("    {:<58} {:>6}  verdict", "reading", "pairs");
    for reading in Reading::ALL {
        let n = r.pairs(reading).len();
        println!("    {:<58} {:>6}  {}", reading.label(), n, verdict(n));
    }
    println!(
        "    {:<58} {:>6}  (the baseline)",
        "S-411 as recorded (84 dirs, shipped reader)",
        r.s411_pairs().len()
    );
    println!("\n    the STRICT pairs, enumerated:");
    for (a, b) in r.pairs(Reading::Strict) {
        println!("      {a:<36} -> {b}");
    }
}

fn report_gained_and_lost(r: &Remeasure, reading: Reading) {
    let now = r.pairs(reading);
    let before = r.s411_pairs();
    println!("\n  {} — versus S-411's {}", reading.label(), before.len());
    println!("    gained:");
    for (a, b) in now.difference(&before) {
        let evidence = r.evidence(a, b);
        let overlays: BTreeSet<&str> = evidence.iter().map(|t| t.overlay.as_str()).collect();
        println!("      {a} -> {b}    overlays {overlays:?}");
        for t in evidence {
            println!(
                "          {} = {:?}  [{}]  {}",
                t.via_key,
                t.value,
                t.source.label(),
                t.file
            );
        }
    }
    println!("    lost:");
    for (a, b) in before.difference(&now) {
        println!(
            "      {a} -> {b}    ({})",
            reading.why_not(a, b, &r.manifest)
        );
    }
}

fn report_sensitivity(r: &Remeasure) {
    println!(
        "\n  SENSITIVITY — a manifest-scoped identity registry (NEVER counted): {} pair(s) \
         STRICT would add",
        r.sensitivity.len()
    );
    for (a, b, label) in &r.sensitivity {
        println!("      {a} -> {b}    label `{label}` stops colliding without the non-member");
    }
}

fn report_coverage(r: &Remeasure) {
    for (name, c) in [
        ("deploy overlays", &r.deploy_coverage),
        ("application config", &r.application_coverage),
    ] {
        println!("\n  THE READER'S COVERAGE — {name}");
        println!(
            "    sequences with a parent key          {:>6}",
            c.sequences
        );
        println!(
            "    items bound                          {:>6}   ({} mapping · {} scalar)",
            c.items_bound(),
            c.mapping_items_bound,
            c.scalar_items_bound
        );
        println!(
            "    items skipped by shape               {:>6}",
            c.items_skipped()
        );
        for (shape, n) in &c.skipped {
            println!(
                "      {:<44} {n:>6}   e.g. {}",
                shape.label(),
                c.examples.get(shape).map_or("", String::as_str)
            );
        }
        println!(
            "    item-scoped keys bound               {:>6}",
            c.keys_bound
        );
        println!(
            "    `-` lines nested inside bound items  {:>6}   (rule 3: unbound)",
            c.nested_lines_in_items
        );
    }
    let new_by_source = |s: SourceSet| {
        r.item_targets
            .iter()
            .filter(|t| t.target.source == s)
            .count()
    };
    println!(
        "\n    item-scoped target values: {} deploy overlay · {} application config",
        new_by_source(SourceSet::Deploy),
        new_by_source(SourceSet::Application)
    );
    println!(
        "    collapsed keys: {}  (any one fails the run)",
        r.collapsed.len()
    );
    for c in r.collapsed.iter().take(20) {
        println!("      {c}");
    }
    report_refusals(r);

    let (matched, unmatched, extra) = reconcile_ceiling(&r.ceiling, &r.bound_hosts);
    println!("\n  RECONCILIATION AGAINST S-411'S {S411_CEILING_LINES} CEILING LINES");
    println!(
        "    ceiling lines (S-411 / this walk)    {:>6} / {}",
        r.s411.sequence_host_ceiling.len(),
        r.ceiling.len()
    );
    println!("    matched by a bound host scalar       {matched:>6}");
    println!(
        "    NOT bound by the reader              {:>6}   ({} of them a Helm template expression)",
        unmatched.len(),
        templated(&unmatched)
    );
    for line in &unmatched {
        println!("      {:<60} {}", line.file, line.value);
    }
    println!(
        "    bound hosts no ceiling line counted  {:>6}",
        extra.len()
    );
    for b in extra.iter().take(20) {
        println!("      {:<60} {} = {}", b.file, b.key, b.value);
    }
}

/// The refused sources, enumerated, and what refusing them forgoes — judged by
/// the same classifier, so "costs nothing" is a measured statement.
fn report_refusals(r: &Remeasure) {
    println!(
        "\n  SOURCES REFUSED — the rewrite did not reproduce the shipped reading: {}",
        r.refused.len()
    );
    for (file, refusal) in &r.refused {
        println!(
            "      {file}   ({} divergence(s), {} item key(s) forgone; first: {})",
            refusal.divergences.len(),
            refusal.forgone.len(),
            refusal.divergences.first().map_or("", String::as_str),
        );
    }
    let forgone_pairs = addressed_pairs(r.forgone_targets.iter(), Reading::Directory, &r.manifest);
    println!(
        "    what the refusals forgo: {} item key(s) · {} target value(s) · {} addressed pair(s) \
         on DIRECTORY {:?}",
        r.refused
            .iter()
            .map(|(_, refusal)| refusal.forgone.len())
            .sum::<usize>(),
        r.forgone_targets.len(),
        forgone_pairs.len(),
        forgone_pairs,
    );
}

fn report_walk(r: &Remeasure) {
    println!(
        "\n  THE GATE'S OWN WALK DENOMINATOR (S-411's RECORDED_CORPUS_ENTRIES is not asserted)"
    );
    println!(
        "    corpus walk  hidden(true)           {:>6} files",
        r.cost.corpus_entries
    );
    println!(
        "    overlay walk hidden(false)          {:>6} files",
        r.cost.overlay_entries
    );
    println!(
        "    deploy-shaped files read             {:>6}",
        r.cost.files_read
    );
    println!(
        "      of those, inside a hidden dir      {:>6}",
        r.cost.files_in_hidden
    );
    println!(
        "    skipped as documentation             {:>6}",
        r.cost.documentation_files
    );
    println!(
        "    application YAML sources re-read     {:>6}",
        r.application_sources
    );
}

// ── The gate ────────────────────────────────────────────────────────────────

/// **S-475's blocking gate.** Reports VOID — and asserts nothing — when no
/// estate is configured, so `cargo test --workspace` stays green without one.
#[test]
fn measure_sequence_addressed_pairs_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!("{}", headline(None));
        return;
    };
    let r = remeasure(&root);
    report(&r);
    println!("\n{RECORDED_FINDING}");

    assert_the_population_is_the_declared_one(&r);
    assert_estate_engaged(&r);
    assert!(
        r.collapsed.is_empty(),
        "{} collapsed key(s) — the sequence reading fabricated or altered a key, which fails \
         the run whatever the verdict:\n{}",
        r.collapsed.len(),
        r.collapsed.join("\n"),
    );

    let strict = r.pairs(Reading::Strict).len();
    println!(
        "\n{}\n  beside it: MANIFEST {} ({}) · DIRECTORY {} ({})",
        headline(Some(strict)),
        r.pairs(Reading::Manifest).len(),
        verdict(r.pairs(Reading::Manifest).len()),
        r.pairs(Reading::Directory).len(),
        verdict(r.pairs(Reading::Directory).len()),
    );
    assert_the_recorded_verdict(&r);
}

fn assert_the_population_is_the_declared_one(r: &Remeasure) {
    let declared = declared_members();
    assert_eq!(
        r.manifest,
        declared,
        "the live manifest's members differ from the population declared before the run \
         (only in manifest: {:?}; only in declaration: {:?}) — the decisive population has \
         moved, so this run measures a different gate",
        r.manifest.difference(&declared).collect::<Vec<_>>(),
        declared.difference(&r.manifest).collect::<Vec<_>>(),
    );
}

/// The estate must have engaged, and the reconciliation must be against the
/// ceiling S-411 recorded — a run that walked nothing prints "0 gained" exactly
/// like a run that walked everything and gained nothing.
fn assert_estate_engaged(r: &Remeasure) {
    assert!(
        r.s411.members.len() >= 80,
        "only {} directories — not the estate",
        r.s411.members.len()
    );
    assert!(
        r.cost.files_read >= 100 && r.cost.files_in_hidden > 0,
        "the overlay walk read {} deploy files ({} in hidden dirs) — it did not engage",
        r.cost.files_read,
        r.cost.files_in_hidden,
    );
    assert!(
        r.application_sources >= 100,
        "only {} application YAML sources re-read",
        r.application_sources
    );
    assert_eq!(
        r.s411.sequence_host_ceiling.len(),
        S411_CEILING_LINES,
        "S-411's ceiling moved from the {S411_CEILING_LINES} lines its finding records; the \
         reconciliation is against a different population",
    );
    assert_eq!(
        r.ceiling.len(),
        r.s411.sequence_host_ceiling.len(),
        "this run's walk found {} ceiling lines where S-411's found {} — the same walk over \
         the same files must agree",
        r.ceiling.len(),
        r.s411.sequence_host_ceiling.len(),
    );
    assert!(
        r.deploy_coverage.items_bound() > 0,
        "the reader bound no sequence item in any deploy file — it did not engage",
    );
}

/// The three readings this run measured, pinned so a drift in the reader, the
/// corpus or the classifier is a failure rather than a quietly different
/// verdict. See `sequence_addressed_pairs_finding.txt`.
pub const RECORDED_STRICT_PAIRS: usize = 11;
pub const RECORDED_MANIFEST_PAIRS: usize = 12;
pub const RECORDED_DIRECTORY_PAIRS: usize = 13;

/// Pairs STRICT gains over S-411 — the two sequence pairs, by count; the names
/// are asserted beside it.
pub const RECORDED_STRICT_GAINED: usize = 2;

/// Pairs a manifest-scoped identity registry would add on STRICT. Never
/// counted; pinned because the finding's statement of what the classifier
/// choice costs is read off it.
pub const RECORDED_SENSITIVITY_PAIRS: usize = 2;

/// `(sources refused, target values those refusals forgo)`. The second being
/// zero is the finding's evidence that the refusal cannot move the verdict.
pub const RECORDED_REFUSALS: (usize, usize) = (32, 0);

/// `(ceiling lines matched by a bound host, unbound, unbound that are Helm
/// template expressions)` — the reconciliation against S-411's 84.
pub const RECORDED_RECONCILIATION: (usize, usize, usize) = (20, 64, 64);

/// Sequence items the reader bound across the deploy overlays, and across the
/// application config — the reader's own coverage, pinned so a reader that
/// quietly stopped engaging is a failure.
pub const RECORDED_ITEMS_BOUND: (usize, usize) = (9742, 7);

const _: () = {
    assert!(
        RECORDED_STRICT_PAIRS < ADDRESSED_PAIR_FLOOR,
        "the recorded finding is FALSIFIED on the decisive reading; if that changes, \
         re-decide CR-153 and ADR-65 rather than relaxing the assertion",
    );
    assert!(
        RECORDED_MANIFEST_PAIRS >= ADDRESSED_PAIR_FLOOR
            && RECORDED_MANIFEST_PAIRS == RECORDED_STRICT_PAIRS + 1,
        "the finding states MANIFEST holds ONLY by counting the test-harness consumer",
    );
};

/// The recorded finding, pinned: the three readings, the pairs gained by name,
/// and every adjudication figure the finding leans on.
fn assert_the_recorded_verdict(r: &Remeasure) {
    let got = (
        r.pairs(Reading::Strict).len(),
        r.pairs(Reading::Manifest).len(),
        r.pairs(Reading::Directory).len(),
    );
    assert_eq!(
        got,
        (
            RECORDED_STRICT_PAIRS,
            RECORDED_MANIFEST_PAIRS,
            RECORDED_DIRECTORY_PAIRS
        ),
        "(STRICT, MANIFEST, DIRECTORY) moved from the recorded figures to {got:?} without the \
         finding being re-recorded — re-decide CR-153 and ADR-65, do not relax this",
    );
    let strict = r.pairs(Reading::Strict);
    let before = r.s411_pairs();
    let gained: Vec<_> = strict.difference(&before).copied().collect();
    assert_eq!(
        gained,
        [
            ("hermodr-gateway", "webmail"),
            ("mailbox-aggregate-hermodr-gateway", "mailbox-api")
        ],
        "the pairs STRICT gains over S-411 are not the two the finding records",
    );
    assert_eq!(gained.len(), RECORDED_STRICT_GAINED);
    assert_eq!(
        r.sensitivity.len(),
        RECORDED_SENSITIVITY_PAIRS,
        "sensitivity moved"
    );
    assert_eq!(
        (r.refused.len(), r.forgone_targets.len()),
        RECORDED_REFUSALS,
        "refused sources / forgone target values moved — if the second is non-zero the \
         refusal can move the verdict and the finding's claim that it cannot is wrong",
    );
    let (matched, unmatched, _) = reconcile_ceiling(&r.ceiling, &r.bound_hosts);
    assert_eq!(
        (matched, unmatched.len(), templated(&unmatched)),
        RECORDED_RECONCILIATION,
        "the reconciliation against S-411's ceiling moved",
    );
    assert_eq!(
        (
            r.deploy_coverage.items_bound(),
            r.application_coverage.items_bound()
        ),
        RECORDED_ITEMS_BOUND,
        "the reader's own coverage moved",
    );
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// The estate arm runs only where a corpus is configured, so every rule it rests
// on is pinned here on every `cargo test`, each matcher with the near miss it
// must reject.

#[cfg(test)]
mod fixtures {
    use super::*;

    fn keys(r: &ItemReading) -> Vec<&str> {
        r.values.keys().map(String::as_str).collect()
    }

    fn value<'a>(r: &'a ItemReading, key: &str) -> Vec<&'a str> {
        r.values
            .get(key)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    fn skipped(r: &ItemReading, shape: Shape) -> usize {
        r.coverage.skipped.get(&shape).copied().unwrap_or(0)
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    // ── The declaration ─────────────────────────────────────────────────────

    #[test]
    fn the_floor_and_the_population_are_the_declared_ones() {
        // Reads the DECLARATION, never the constant against itself, and requires
        // the floor line to be UNIQUE — S-411's review showed a second `>= NN`
        // line could otherwise redefine the floor under a green test.
        let hits: Vec<usize> = DECLARED_FLOOR
            .lines()
            .filter(|l| l.contains("ADDRESSED MEMBER PAIRS"))
            .filter_map(|l| {
                l.trim()
                    .strip_prefix(">= ")?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        assert_eq!(
            hits,
            vec![ADDRESSED_PAIR_FLOOR],
            "the declaration must state the floor on exactly one `>= NN ADDRESSED MEMBER PAIRS` \
             line, equal to ADDRESSED_PAIR_FLOOR"
        );
        // The floor is CR-131's, unrevised: the verbatim B1 quote carries it, and
        // S-411's constant for the same metric agrees.
        let quoted: Vec<usize> = DECLARED_FLOOR
            .split("tracked file: ")
            .skip(1)
            .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
            .collect();
        assert_eq!(
            quoted,
            vec![ADDRESSED_PAIR_FLOOR],
            "CR-131 B1's quoted floor disagrees"
        );
        assert_eq!(
            ADDRESSED_PAIR_FLOOR,
            crate::config_declared_coupling::ADDRESSED_PAIR_FLOOR,
            "the floor must be S-411's, unrevised"
        );
        assert!(DECLARED_FLOOR.contains(
            "**Floors, declared here and parsed by the test from a tracked file: 12 addressed \
             pairs and 10 shared topics.**"
        ));
        assert!(
            DECLARED_FLOOR.contains("2026-09-29T09:17:07Z"),
            "the declaration's UTC stamp"
        );
        assert!(
            DECLARED_FLOOR.contains("logos commit 5e3d0547"),
            "the named commit"
        );
        assert!(DECLARED_FLOOR.contains("THE DECISIVE READING IS STRICT"));

        let members = declared_members();
        assert!(
            DECLARED_FLOOR.contains(&format!("MANIFEST MEMBERS — BEGIN ({})", members.len())),
            "the member list parses to {} names, not the count its BEGIN marker states",
            members.len()
        );
        assert_eq!(members.len(), 83);
        assert!(
            !members.contains("archive-api-logiclens-fork"),
            "the fork is excluded"
        );
        assert!(members.contains("archive-api") && members.contains(TEST_HARNESS_CONSUMER));
        assert!(DECLARED_FLOOR.contains(TEST_HARNESS_CONSUMER));
        assert!(DECLARED_FLOOR.contains(&format!("S-411's {S411_CEILING_LINES} ceiling lines")));
    }

    // ── The reader ──────────────────────────────────────────────────────────

    /// The incident the shipped flattener's doc comment records: a `routes:`
    /// list that fabricated `spring.cloud.gateway.routes.uri`.
    const ROUTES: &str = "\
spring:
  cloud:
    gateway:
      routes:
      - id: mail
        uri: http://mailbox-api:8080
        predicates:
        - Path=/mail/**
      - id: box
        uri: lb://box
  application:
    name: gateway
";

    #[test]
    fn the_routes_list_binds_under_its_items_and_never_under_the_list() {
        let r = read_items(ROUTES);
        assert_eq!(
            keys(&r),
            [
                "spring.cloud.gateway.routes[0].id",
                "spring.cloud.gateway.routes[0].uri",
                "spring.cloud.gateway.routes[1].id",
                "spring.cloud.gateway.routes[1].uri",
            ]
        );
        assert_eq!(
            value(&r, "spring.cloud.gateway.routes[0].uri"),
            ["http://mailbox-api:8080"]
        );
        assert!(!r
            .values
            .keys()
            .any(|k| k == "spring.cloud.gateway.routes.uri"));
        // The item's nested `predicates:` list stays unbound (rule 3) …
        assert!(!r.values.keys().any(|k| k.contains("predicates")));
        assert_eq!(r.coverage.nested_lines_in_items, 1);
        // … and the mapping after the list is the shipped parser's, untouched.
        let shipped = parse_yaml(ROUTES);
        assert_eq!(
            shipped.keys().collect::<Vec<_>>(),
            ["spring.application.name"]
        );
        assert!(r.collapsed.is_empty(), "{:?}", r.collapsed);
        assert_eq!(
            (r.coverage.sequences, r.coverage.mapping_items_bound),
            (1, 2)
        );
        assert_eq!(r.coverage.keys_bound, 4);
    }

    /// The estate's dominant shape: a compact sequence (`- ` at the key's own
    /// indent) of upstream mappings, each carrying its own `port`.
    const UPSTREAMS: &str = "\
proxy:
  mode: userinfoflow
  upstreams:
  - name: mailbox-api
    default: true
    host: mailbox-api.pec-services.svc.cluster.local
    port: 9000
    request:
      urlrewrite:
        path: /v1/users
  - name: other
    host: other-api.pec-services.svc.cluster.local
log:
  level: trace
";

    #[test]
    fn a_mapping_item_binds_each_scalar_under_its_own_index() {
        let r = read_items(UPSTREAMS);
        assert_eq!(
            keys(&r),
            [
                "proxy.upstreams[0].default",
                "proxy.upstreams[0].host",
                "proxy.upstreams[0].name",
                "proxy.upstreams[0].port",
                "proxy.upstreams[0].request.urlrewrite.path",
                "proxy.upstreams[1].host",
                "proxy.upstreams[1].name",
            ]
        );
        assert_eq!(value(&r, "proxy.upstreams[1].name"), ["other"]);
        assert!(r.collapsed.is_empty(), "{:?}", r.collapsed);

        let indented = read_items("proxy:\n  upstreams:\n    - name: a\n      host: a-svc.ns\n");
        assert_eq!(
            keys(&indented),
            ["proxy.upstreams[0].host", "proxy.upstreams[0].name"]
        );
    }

    #[test]
    fn a_host_is_admitted_only_beside_a_port_in_the_same_item() {
        // Through S-411's own admission rule. Item 0's `port` is not a sibling
        // of item 1's `host` — the near miss an index-free key would get wrong,
        // since `proxy.upstreams.host` and `proxy.upstreams.port` WOULD be
        // siblings.
        let r = read_items(UPSTREAMS);
        let (mut scalars, mut targets) = (Vec::new(), Vec::new());
        collect_values(
            &Provenance {
                member: "gw",
                file: "gw/values.yaml",
                overlay: "o",
                source: SourceSet::Deploy,
            },
            &r.values,
            &mut scalars,
            &mut targets,
        );
        let admitted: Vec<(&str, &str)> = targets
            .iter()
            .map(|t| (t.via_key.as_str(), t.label.as_str()))
            .collect();
        assert_eq!(admitted, [("proxy.upstreams[0].host", "mailbox-api")]);
    }

    #[test]
    fn a_scalar_sequence_binds_under_key_index_and_a_url_is_not_a_mapping() {
        let r = read_items(
            "cors:\n  allowed-origins:\n  - http://mailbox-api:8080\n  - \"https://webmail.example/x\"\n  - plain # c\n",
        );
        assert_eq!(
            keys(&r),
            [
                "cors.allowedorigins[0]",
                "cors.allowedorigins[1]",
                "cors.allowedorigins[2]"
            ]
        );
        assert_eq!(
            value(&r, "cors.allowedorigins[0]"),
            ["http://mailbox-api:8080"]
        );
        assert_eq!(
            value(&r, "cors.allowedorigins[1]"),
            ["https://webmail.example/x"]
        );
        assert_eq!(value(&r, "cors.allowedorigins[2]"), ["plain"]);
        // The near miss: the shipped key split cuts at the FIRST `:`, which would
        // read `{http: //mailbox-api:8080}` and invent `…[0].http`.
        assert!(!r.values.keys().any(|k| k.ends_with(".http")));
        assert_eq!(r.coverage.scalar_items_bound, 3);
        // `- Error: x` IS a mapping in YAML.
        assert_eq!(keys(&read_items("e:\n- Error: x\n")), ["e[0].error"]);
    }

    #[test]
    fn an_escaped_quote_never_ends_an_item_key() {
        // One double-quoted string, not a mapping: its first `"` is escaped.
        let r = read_items("l:\n- \"u\\\": http://mailbox-api/x\"\n");
        assert!(r.values.is_empty(), "{:?}", r.values);
        assert_eq!(skipped(&r, Shape::NothingRead), 1);
        // The near miss: the same shape without the escape IS a mapping.
        assert_eq!(keys(&read_items("l:\n- \"u\": http://mailbox-api/x\n")), ["l[0].u"]);
    }

    #[test]
    fn a_mapping_item_whose_key_is_not_one_token_binds_nothing() {
        // `{"a b": c}` in YAML — never the scalar `"a b: c"`, and never a URL.
        for text in ["l:\n- a b: c\n", "l:\n- http://mailbox-api/a b: c\n"] {
            let r = read_items(text);
            assert!(r.values.is_empty(), "{text:?} -> {:?}", r.values);
            assert_eq!(skipped(&r, Shape::Unreadable), 1, "{text:?}");
        }
        // The near miss: no value colon, so a plain scalar.
        assert_eq!(value(&read_items("l:\n- a b:c\n"), "l[0]"), ["a b:c"]);
    }

    #[test]
    fn every_skip_shape_under_reads_and_says_why() {
        // Each row turns a would-be binding into a counted skip. Deleting any
        // one of these rules would bind text the shipped parser never reads.
        for (text, shape) in [
            ("l:\n- *ref\n", Shape::AnchorOrAlias),
            ("l:\n- &a\n  k: v\n", Shape::AnchorOrAlias),
            ("l:\n- http://mailbox-api\n  /x\n", Shape::MultiLineScalar),
            ("l:\n-x\n", Shape::Unreadable),
            ("l:\n- \"a:b\": c\n", Shape::Unreadable),
        ] {
            let r = read_items(text);
            assert!(r.values.is_empty(), "{text:?} -> {:?}", r.values);
            assert_eq!(skipped(&r, shape), 1, "{text:?} -> {:?}", r.coverage);
        }
        // A quoted scalar carrying `: ` is a scalar, not a mapping.
        assert_eq!(value(&read_items("l:\n- \"x: y\"\n"), "l[0]"), ["x: y"]);
    }

    #[test]
    fn coverage_absorbs_every_field_and_keeps_the_first_example() {
        let one = |n: usize, e: &str| Coverage {
            sequences: n,
            mapping_items_bound: n,
            scalar_items_bound: n,
            skipped: BTreeMap::from([(Shape::NothingRead, n)]),
            keys_bound: n,
            nested_lines_in_items: n,
            examples: BTreeMap::from([(Shape::NothingRead, e.to_string())]),
        };
        let mut total = one(1, "first");
        total.absorb(&one(2, "second"));
        assert_eq!(
            total,
            Coverage {
                sequences: 3,
                mapping_items_bound: 3,
                scalar_items_bound: 3,
                skipped: BTreeMap::from([(Shape::NothingRead, 3)]),
                keys_bound: 3,
                nested_lines_in_items: 3,
                examples: BTreeMap::from([(Shape::NothingRead, "first".to_string())]),
            }
        );
    }

    #[test]
    fn a_nested_sequence_stays_unbound_and_keeps_its_siblings_positions() {
        let r = read_items("matrix:\n- - a\n  - b\n- name: x\n  tags:\n  - t1\n");
        // Item 1 is still `[1]`: the index is the item's POSITION, never the
        // count of items bound before it.
        assert_eq!(keys(&r), ["matrix[1].name"]);
        assert_eq!(skipped(&r, Shape::NestedSequence), 1);
        assert_eq!(r.coverage.nested_lines_in_items, 1);
        assert!(r.collapsed.is_empty(), "{:?}", r.collapsed);
    }

    #[test]
    fn a_flow_sequence_or_mapping_item_stays_unbound() {
        let text = "hosts:\n- [a, b]\n- {k: v}\nlist: [c, d]\nafter: ok\n";
        let r = read_items(text);
        assert!(r.values.is_empty(), "{:?}", r.values);
        assert_eq!(skipped(&r, Shape::FlowSequence), 1);
        assert_eq!(skipped(&r, Shape::FlowMapping), 1);
        assert_eq!(parse_yaml(text).keys().collect::<Vec<_>>(), ["after"]);
        assert!(r.collapsed.is_empty(), "{:?}", r.collapsed);
    }

    #[test]
    fn a_dash_inside_a_block_scalar_is_text_not_a_sequence() {
        let r = read_items(
            "script: |\n  - host: evil.example\n  upstreams:\n  - host: x\nhooks:\n- |\n  body\nafter:\n  - host: real-svc.ns\n    port: 1\n",
        );
        assert_eq!(keys(&r), ["after[0].host", "after[0].port"]);
        assert_eq!(skipped(&r, Shape::BlockScalar), 1);
        // The keys alone cannot tell: the shipped parser swallows the block
        // body whatever the reader does with it. The COVERAGE can — a reader
        // that took the body's `upstreams:` list for a sequence would report a
        // third sequence and an item it "could not read".
        assert_eq!(r.coverage.sequences, 2, "{:?}", r.coverage);
        assert_eq!(r.coverage.items_skipped(), 1, "{:?}", r.coverage);
    }

    #[test]
    fn a_sequence_with_no_parent_key_binds_nothing() {
        let r = read_items("- name: a\n  host: b.ns\n---\nk: v\n- x\n");
        assert!(r.values.is_empty(), "{:?}", r.values);
        assert_eq!(skipped(&r, Shape::NoParentKey), 2);
        assert_eq!(r.coverage.sequences, 0);
    }

    #[test]
    fn a_comment_between_items_does_not_end_the_sequence() {
        let r = read_items("l:\n- a\n# note\n- b\n\n- c\n");
        assert_eq!(keys(&r), ["l[0]", "l[1]", "l[2]"]);
        assert_eq!(r.coverage.sequences, 1);
    }

    #[test]
    fn a_source_carrying_the_sentinel_is_left_unread() {
        let r = read_items("zzlogosseqitem0zz:\n- a\nx:\n- b\n");
        assert!(r.values.is_empty());
        assert_eq!(skipped(&r, Shape::SentinelInSource), 1);
    }

    // ── The collapsed-key check ─────────────────────────────────────────────

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (k, v) in pairs {
            out.entry((*k).to_string())
                .or_default()
                .insert((*v).to_string());
        }
        out
    }

    #[test]
    fn an_index_free_key_from_a_sequence_fails_the_run() {
        let ok = map(&[("proxy.upstreams[0].host", "h")]);
        assert!(collapsed_keys(&ok).is_empty());
        let collapsed = map(&[
            ("proxy.upstreams[0].host", "h"),
            ("proxy.upstreams.host", "h"),
        ]);
        assert_eq!(collapsed_keys(&collapsed).len(), 1);
    }

    #[test]
    fn a_leaked_or_altered_shipped_key_is_a_divergence() {
        let shipped = map(&[("proxy.mode", "x")]);
        assert!(divergent_keys(&shipped, &shipped).is_empty());
        // An item line registering under its enclosing mapping — the incident.
        let leaked = map(&[("proxy.mode", "x"), ("proxy.upstreams.host", "h")]);
        assert_eq!(divergent_keys(&shipped, &leaked).len(), 1);
        let altered = map(&[("proxy.mode", "y")]);
        assert_eq!(divergent_keys(&shipped, &altered).len(), 1);
        assert_eq!(divergent_keys(&shipped, &BTreeMap::new()).len(), 1);
    }

    /// The estate's Helm template shape: a column-0 directive cuts the
    /// container item in two. The shipped parser hangs `image:` under
    /// `containers` (its own `routes:`-class fabrication); a rewrite cannot
    /// scope that item as the shipped parser bounds it, so the source is
    /// refused whole — nothing added, nothing collapsed, the cost kept.
    const HELM_TEMPLATE: &str = "\
spec:
  containers:
    - name: {{ .Chart.Name }}
      env:
{{- range $key, $value := .Values.env }}
        - name: {{ $key }}
{{- end }}
      image: \"repo/app:{{ .Values.tag }}\"
      ports:
        - name: http
          containerPort: 8080
";

    #[test]
    fn a_source_the_rewrite_cannot_scope_faithfully_is_refused_whole() {
        assert!(
            parse_yaml(HELM_TEMPLATE).contains_key("spec.containers.image"),
            "the fixture must reproduce the shipped parser's own fabrication"
        );
        let r = read_items(HELM_TEMPLATE);
        assert!(r.values.is_empty(), "{:?}", r.values);
        assert!(
            r.collapsed.is_empty(),
            "a refusal is not a collapsed key: {:?}",
            r.collapsed
        );
        let refusal = r.refused.as_ref().expect("refused");
        assert!(!refusal.divergences.is_empty());
        assert!(
            refusal.forgone.contains_key("spec.containers[0].image"),
            "{:?}",
            refusal.forgone
        );
        // Every item the source held — the cut container item, the orphaned
        // env item after the directive, and the ports item — counted once.
        assert_eq!(skipped(&r, Shape::SourceRefused), 3, "{:?}", r.coverage);
        // The near miss: the same item without the column-0 directive is read.
        let plain = read_items("spec:\n  containers:\n    - name: app\n      image: repo/app\n");
        assert!(plain.refused.is_none());
        assert_eq!(
            keys(&plain),
            ["spec.containers[0].image", "spec.containers[0].name"]
        );
    }

    #[test]
    fn item_scoped_means_a_named_key_with_a_numeric_index() {
        assert!(is_item_scoped("proxy.upstreams[0].host"));
        assert!(is_item_scoped("cors.allowedorigins[12]"));
        for near_miss in [
            "proxy.upstreams.host",
            "[0].host",
            "a.b[x].c",
            "a.b[].c",
            "a.zzlogosseqitem0zz.host",
            "a.b[0.c",
        ] {
            assert!(!is_item_scoped(near_miss), "{near_miss}");
        }
    }

    // ── Populations, verdict, VOID ──────────────────────────────────────────

    fn judged(a: &str, b: &str, outcome: PairOutcome) -> JudgedTarget {
        JudgedTarget {
            target: Target {
                member: a.to_string(),
                label: b.to_string(),
                form: super::super::config_declared_coupling::TargetForm::Url,
                source: SourceSet::Deploy,
                overlay: "o".to_string(),
                via_key: "k".to_string(),
                file: "f".to_string(),
                value: "v".to_string(),
                port: None,
            },
            outcome,
            provider: Some(b.to_string()),
        }
    }

    #[test]
    fn each_reading_counts_its_own_population() {
        let manifest = set(&["a", "b", "c", TEST_HARNESS_CONSUMER]);
        let targets = [
            judged("a", "b", PairOutcome::Addressed),
            judged("a", "b", PairOutcome::Addressed),
            judged("fork", "b", PairOutcome::Addressed),
            judged("a", "fork", PairOutcome::Addressed),
            judged(TEST_HARNESS_CONSUMER, "b", PairOutcome::Addressed),
            judged("a", "c", PairOutcome::PathOnlyMatched),
        ];
        let n = |r: Reading| addressed_pairs(targets.iter(), r, &manifest).len();
        assert_eq!(
            (
                n(Reading::Directory),
                n(Reading::Manifest),
                n(Reading::Strict)
            ),
            (4, 2, 1)
        );
        assert_eq!(
            Reading::Manifest.why_not("fork", "b", &manifest),
            "consumer is not a manifest member"
        );
        assert_eq!(
            Reading::Manifest.why_not("a", "fork", &manifest),
            "provider is not a manifest member"
        );
        assert_eq!(
            Reading::Strict.why_not(TEST_HARNESS_CONSUMER, "b", &manifest),
            "test-harness consumer"
        );
    }

    #[test]
    fn only_the_strict_reading_decides_and_a_blind_run_is_void() {
        assert_eq!(verdict(ADDRESSED_PAIR_FLOOR - 1), "FALSIFIED");
        assert_eq!(verdict(ADDRESSED_PAIR_FLOOR), "HOLDS");
        let void = headline(None);
        assert!(void.starts_with("VOID"), "{void}");
        assert!(!void.contains("HOLDS") && !void.contains("FALSIFIED") && !void.contains(" 0 "));
        assert!(headline(Some(ADDRESSED_PAIR_FLOOR)).contains("STRICT reading 12"));
        assert!(headline(Some(ADDRESSED_PAIR_FLOOR)).ends_with("HOLDS"));
    }

    #[test]
    fn only_a_helm_expression_counts_as_templated() {
        // The finding's "every unbound ceiling line is a template" is read off
        // this count; on the estate it equals the unbound total, so only a
        // fixture can tell the filter from a plain length.
        let line = |v: &str| SequenceHost {
            member: "m".to_string(),
            file: "f".to_string(),
            value: v.to_string(),
        };
        let (templated_line, plain_line) = (line("{{ .host | quote }}"), line("mailbox-api.ns"));
        assert_eq!(templated(&[&templated_line, &plain_line]), 1);
        assert_eq!(templated(&[&plain_line]), 0);
    }

    #[test]
    fn the_ceiling_reconciles_line_by_line_within_one_file() {
        let line = |f: &str, v: &str| SequenceHost {
            member: "m".to_string(),
            file: f.to_string(),
            value: v.to_string(),
        };
        let host = |f: &str, v: &str| BoundHost {
            file: f.to_string(),
            key: "k[0].host".to_string(),
            value: v.to_string(),
        };
        let ceiling = [line("f1", "h1"), line("f1", "h1"), line("f2", "h2")];
        // Same value in another file is not a match — the near miss.
        let bound = [host("f1", "h1"), host("f3", "h2")];
        let (matched, unmatched, extra) = reconcile_ceiling(&ceiling, &bound);
        assert_eq!((matched, unmatched.len(), extra.len()), (1, 2, 1));
        // …and a different value in the same file is not a match either.
        let other_value = [host("f2", "h9")];
        let (matched, unmatched, extra) = reconcile_ceiling(&ceiling, &other_value);
        assert_eq!((matched, unmatched.len(), extra.len()), (0, 3, 1));
    }
}
