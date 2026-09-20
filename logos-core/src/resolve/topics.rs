//! The **broker-topic promotion pass** — Pass 2¾ of the pipeline
//! ([resolution-engine], S-256, [FR-WS-11], [ADR-55]).
//!
//! Runs after the framework-promotion pass on every index/sync and promotes the
//! **ledger-only** broker references S-254 captured ([FR-WS-10]) to the
//! first-class graph entities migration 17 admitted (S-255, [FR-WS-11]):
//!
//! - each distinct topic key becomes one [`NodeKind::Topic`] node;
//! - each `(publishing declaration, topic)` becomes a [`NodeKind::Producer`]
//!   node linked to its topic by [`EdgeKind::Publishes`];
//! - each `(subscribing declaration, topic)` becomes a [`NodeKind::Consumer`]
//!   node linked to its topic by [`EdgeKind::Subscribes`].
//!
//! # The ledger is the input — no second capture
//! The pass **re-reads no source from disk**. S-254's `brokers.scm` interpreter
//! ([`crate::extract::broker`]) already normalized every static-topic
//! publish/subscribe site into an `unresolved_refs` row tagged
//! [`BrokerPublish`](ArtifactRelation::BrokerPublish) /
//! [`BrokerSubscribe`](ArtifactRelation::BrokerSubscribe), so promotion is a pure
//! function of that ledger, the bound graph and — since [S-424] — the member's
//! **own committed configuration**, which is the one thing it does read
//! ([FR-WS-27] AC4). That read is a key lookup in this member's own store, not a
//! re-extraction and not a reach into another member's: a topic operand that
//! names `${spring.kafka.topics.orders}` is keyed by the value this repository
//! commits for it. A dynamically-composed topic was already refused at capture
//! ([NFR-RA-05]) and can never reach this pass, so no topic is ever fabricated
//! here — an operand the corpus proves nothing for keeps its placeholder exactly
//! as written.
//!
//! # Identity: a topic is repo-scoped, a producer/consumer is site-scoped
//! A [`Topic`](NodeKind::Topic) is the *shared* identity two sides meet on, so its
//! symbol carries **no file path** (`logos . . . topic/orders#`): two files
//! publishing `orders` share one topic node, which is precisely what makes a
//! per-repo topic graph visible *before* any cross-repo match exists
//! ([FR-WS-11], [ADR-55]). It is also the only promoted node with no `file_id` —
//! a topic is not declared at a line, and anchoring it to an arbitrary one of its
//! call sites would fabricate a location.
//!
//! A [`Producer`](NodeKind::Producer)/[`Consumer`](NodeKind::Consumer) *is* a code
//! site, so it hangs off its enclosing declaration's symbol
//! (`…/OrderService#publish().orders#`) — unique per `(declaration, topic)`, and
//! file-anchored, so re-extracting the file naturally invalidates it.
//!
//! # Reconcile, don't accumulate
//! Like the framework pass, each run recomputes the full desired set from the
//! current ledger and diffs it against what is promoted: missing nodes are
//! inserted, stale ones deleted, survivors keep their ids. The pass is therefore
//! idempotent and self-healing across syncs, and a graph whose last topic
//! disappeared is demoted cleanly.
//!
//! # A no-topic graph is byte-for-byte unaffected ([NFR-RA-06], [FR-WS-11])
//! Every run — cold `index` and incremental `sync` alike — first asks the store for
//! a **broker footprint**: any promoted broker node, or any broker-arm ledger row.
//! A repo with neither (every repo that indexes no broker topics) skips the
//! whole-graph snapshot and writes nothing at all, so its store is bit-identical
//! with and without this pass.
//!
//! # The cross-member bind is *not* here
//! Promotion is per-repo. Binding a producer in one member to a consumer in
//! another rides the same topic identity through the workspace bridge
//! ([`crate::federation::broker`]) — see that module.
//!
//! # One identify function, called from here and from the bridge ([FR-WS-27])
//! The two tiers are projections of one captured fact and they key it by
//! **calling one function**, [`crate::resolve::broker_identity::identify`]. That is
//! why they cannot disagree — and it is a stronger statement than the one this
//! header used to make. From [S-410] until [S-424] it said they were *"keyed
//! identically, which is why they cannot disagree"* while the bridge resolved
//! its operand through committed configuration and this pass took
//! `row.target.trim()` verbatim. They disagreed for every configuration-bound
//! site, splitting a publisher from its subscriber wherever the two spelled one
//! property differently and leaving that coupling unrepresentable. Two tiers
//! stating one rule is what drifted; one function is what does not ([ADR-52],
//! [FR-WS-27] AC1).
//!
//! **No estate figure is recorded here, deliberately.** They live in
//! `tests/broker_topic_corpus.rs`'s recorded finding and in [CR-136]'s delivery
//! appendix — one home each. This file is swept by no figure roster, so a number
//! copied into it is a number that ages silently.
//!
//! [CR-136]: ../../../docs/requests/CR-136-promoted-topic-identity-is-the-committed-value.md
//!
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [FR-WS-11]: ../../../docs/specs/requirements/FR-WS-11.md
//! [ADR-55]: ../../../docs/specs/architecture/decisions/ADR-55.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
//! [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use anyhow::Result;

use crate::extract::symbol::{descriptor_for, SymbolContext};
use crate::graph_store::{NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, BridgeNamespace, EdgeKind, LogosSymbol, NodeId, NodeKind};
use crate::resolve::binding::{config_bound_keys_of, ConfigLookup, MemberCorpus};
use crate::resolve::broker_identity;
use crate::runtime::Runtime;

use super::promote::{self, Promoted, PromotedEdge};

/// What one promotion run did — surfaced for tracing, not for the gated signal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TopicStats {
    /// Distinct topic nodes in the desired set.
    pub topics: u64,
    /// Producer (publish-site) nodes in the desired set.
    pub producers: u64,
    /// Consumer (subscribe-site) nodes in the desired set.
    pub consumers: u64,
    /// Wall-clock of the pass.
    pub duration_ms: u64,
}

/// The node kinds this pass owns and reconciles.
const PROMOTED: [NodeKind; 3] = [NodeKind::Topic, NodeKind::Producer, NodeKind::Consumer];

/// Run the broker-topic promotion pass. See the module docs for the shape.
///
/// # Errors
/// Returns an error if the snapshot read or the commit batch fails (the batch
/// rolls back wholesale, [NFR-RA-07]).
///
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn run(runtime: &Runtime) -> Result<TopicStats> {
    let started = Instant::now();

    // The no-topic fast path (see the module docs): a graph with no broker footprint
    // can promote nothing and has nothing to demote, so the whole-graph snapshot below
    // is provably a no-op. One cheap EXISTS read instead of the O(graph)
    // materialisation.
    //
    // Deliberately **unconditional**, unlike the framework pass's `delta.is_some()`
    // gate: the overwhelming majority of repos index no broker coupling at all, and a
    // cold `index` of one of them has no reason to materialise
    // `indexed_files + all_nodes + all_edges + unresolved_refs` only to discover an
    // empty desired set. The probe is strictly cheaper on both paths.
    if !runtime.submit_read(|store| store.has_broker_footprint())? {
        return Ok(TopicStats {
            duration_ms: elapsed_ms(started),
            ..TopicStats::default()
        });
    }
    // One consistent snapshot, the same basis the framework pass reconciles from.
    let (files, nodes, edges, refs) = runtime.submit_read(|store| {
        Ok((
            store.indexed_files()?,
            store.all_nodes()?,
            store.all_edges()?,
            store.unresolved_refs()?,
        ))
    })?;

    let existing: Vec<&NodeRow> = nodes
        .iter()
        .filter(|n| PROMOTED.contains(&n.kind))
        .collect();

    let file_id_by_path: HashMap<&str, i64> =
        files.iter().map(|f| (f.path.as_str(), f.id)).collect();
    let corpus = member_corpus(runtime, &refs)?;
    let desired = desired_set(&refs, &nodes, &file_id_by_path, &corpus);

    // Belt and braces behind the footprint probe: a ledger that carries a broker row
    // whose enclosing declaration is unknown (so it promotes nothing) leaves an empty
    // desired set with nothing promoted. No writer batch is opened at all, so the store
    // stays byte-identical ([FR-WS-11], [NFR-RA-06]).
    if desired.is_empty() && existing.is_empty() {
        return Ok(TopicStats {
            duration_ms: elapsed_ms(started),
            ..TopicStats::default()
        });
    }

    let count = |kind: NodeKind| desired.values().filter(|d| d.kind == kind).count() as u64;
    let (topics, producers, consumers) = (
        count(NodeKind::Topic),
        count(NodeKind::Producer),
        count(NodeKind::Consumer),
    );

    commit(runtime, &existing, &edges, desired)?;
    Ok(TopicStats {
        topics,
        producers,
        consumers,
        duration_ms: elapsed_ms(started),
    })
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// The canonical configuration keys this ledger's **broker** rows name, in
/// source order and de-duplicated — empty when they name none ([FR-WS-27] AC4).
///
/// The gate on whether a corpus is read at all, and it is deliberately the
/// **same predicate** the workspace bridge opens a member's store on
/// ([`config_bound_keys_of`]). That function's own doc records why: a gate that
/// opens a store and a gate that resolves an operand must be one predicate, or a
/// store is opened for an operand nothing resolves — or, worse, an operand
/// resolves in a member whose corpus was never read. Asking the question a
/// second way here would re-create exactly that.
///
/// The arm test lives inside `config_bound_keys_of` and is on the **relation**,
/// so a non-broker row carrying a `${…}` — an HTTP client call's configured
/// base URL, which this pass promotes nothing for — contributes no key and
/// costs this member no read.
///
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
fn broker_config_keys(refs: &[UnresolvedRefRow]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for row in refs {
        let Some(relation) = row.payload.as_deref().and_then(ArtifactRelation::from_wire) else {
            continue;
        };
        // Only the broker arm: `config_bound_keys_of` admits the HTTP arm too,
        // whose configured targets this pass promotes nothing for.
        //
        // Tested on the **namespace**, exactly as `broker_identity::admit` is, and
        // NOT by re-spelling the relation list. A re-spelled list is a second
        // predicate: the day a third relation maps to `BrokerTopic`, `admit` would
        // key the site on its committed value while this gate silently omitted its
        // key from the corpus — one captured fact, two keys, with one identify
        // function in place. That is the drift [FR-WS-27] AC1 exists to forbid,
        // arriving through the corpus rather than through the rule.
        if relation.bridge_namespace() != Some(BridgeNamespace::BrokerTopic) {
            continue;
        }
        let Some(named) = config_bound_keys_of(relation, &row.target) else {
            continue;
        };
        for key in named {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// Read **this member's own** committed configuration for every key its broker
/// rows name ([FR-WS-27] AC4, [ADR-64]'s within-reach rule).
///
/// One read over the whole key set, never one per site, and **no read at all**
/// when the ledger names no key — which is every repository whose broker
/// operands are literals, and every repository that indexes no broker coupling.
/// That is the emptiness test [NFR-PE-10] asks for, and it is why this pass
/// costs an unconfigured repo nothing.
///
/// **No store but the member's own is opened**: `runtime` *is* this member's
/// store, and the pass holds no registry, no roster and no second runtime with
/// which it could reach another. Federation stays an overlay ([ADR-52]) — the
/// value keying a node here is one this repository commits for itself.
///
/// # Errors
/// Propagates a read failure, which aborts the pass rather than promoting nodes
/// keyed on a corpus that was only partly read.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
fn member_corpus(runtime: &Runtime, refs: &[UnresolvedRefRow]) -> Result<MemberCorpus> {
    corpus_for(refs, |keys| {
        runtime.submit_read(move |store| {
            let mut corpus = MemberCorpus::new();
            for key in keys {
                let defs = store.config_definitions(&key)?;
                // A key no source defines is ABSENT rather than present-and-empty:
                // the resolver reads an absent key as `missing`, and an empty vec
                // would say the same thing twice. Mirrors the bridge's own reader.
                if !defs.is_empty() {
                    corpus.insert(key, defs);
                }
            }
            Ok(corpus)
        })
    })
}

/// The read **gate** itself, split from the read so it can be asserted rather
/// than argued ([FR-WS-27] AC4, [NFR-PE-10]).
///
/// `read` is invoked **only** when the ledger names at least one broker
/// configuration key. That is the whole of the emptiness test the criterion asks
/// for, and separating it is what lets a test observe the thing the criterion
/// says — *"only members naming a broker key read a corpus"* — instead of
/// observing the key list and inferring the rest. Before this split the
/// short-circuit could be deleted with the entire suite staying green.
///
/// # Errors
/// Propagates whatever `read` returns.
///
/// [FR-WS-27]: ../../../docs/specs/requirements/FR-WS-27.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
fn corpus_for(
    refs: &[UnresolvedRefRow],
    read: impl FnOnce(Vec<String>) -> Result<MemberCorpus>,
) -> Result<MemberCorpus> {
    let keys = broker_config_keys(refs);
    if keys.is_empty() {
        return Ok(MemberCorpus::new());
    }
    read(keys)
}

/// One node this run wants promoted, keyed in the desired map by its symbol
/// string.
#[derive(Debug, Clone)]
struct DesiredNode {
    symbol: LogosSymbol,
    kind: NodeKind,
    name: String,
    /// The anchoring file — `None` for a [`Topic`](NodeKind::Topic), which is a
    /// repo-scoped identity rather than a declaration at a line.
    file_id: Option<i64>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    edges: Vec<DesiredEdge>,
}

/// One edge a promoted node must carry after this run.
///
/// The two broker edges point at a [`Topic`](NodeKind::Topic) that may itself be
/// created in the *same* commit, so they name their target by **symbol** (resolved
/// to an id inside the writer batch) rather than by [`NodeId`] the way the
/// framework pass's already-bound targets can.
#[derive(Debug, Clone)]
enum DesiredEdge {
    /// `enclosing declaration --Contains--> promoted` (scope anchoring).
    ContainedBy(NodeId),
    /// `producer --Publishes--> topic` ([FR-WS-11]).
    Publishes(String),
    /// `consumer --Subscribes--> topic` ([FR-WS-11]).
    Subscribes(String),
}

/// The topic identity, kind, and enclosing declaration one broker ledger row
/// promotes.
struct BrokerRef<'a> {
    /// `Producer` for a publish, `Consumer` for a subscribe.
    kind: NodeKind,
    /// The topic key this row promotes under: the committed value where the
    /// member's configuration proves one, or the operand exactly as written
    /// where it does not ([FR-WS-27] AC2/AC3).
    ///
    /// Owned rather than borrowed from the row, because a committed value is
    /// composed rather than a slice of the operand. **One row can yield several
    /// of these** — see [`broker_refs`], which flattens an overlay disagreement
    /// into one `BrokerRef` per committed value.
    topic: String,
    /// The declaration the capture attributed the site to.
    enclosing: &'a NodeRow,
    /// 1-based line of the publish/subscribe site.
    line: Option<i64>,
}

/// Project the ledger onto the broker rows this pass promotes, keyed by the
/// **committed-value topic identity** ([FR-WS-27] AC1).
///
/// A row qualifies iff [`broker_identity::identify`] admits it — one function, called
/// from here and from the federation bridge, so a `Topic` node and a bridge edge
/// can never key one captured fact two ways ([ADR-52]). Every refusal that
/// function makes is therefore made here too and in the same words: a non-broker
/// relation and a **keyless** row (the arm's recorded `topic-not-literal`
/// refusal, [CR-107]) promote nothing, and no topic is fabricated
/// ([NFR-RA-05]).
///
/// The relation is matched a second time here, and that is a projection rather
/// than a duplicated predicate: [`NodeKind::Producer`]/[`NodeKind::Consumer`] is
/// this pass's own vocabulary, which no other tier names, while the bridge reads
/// the same relation as a fan-out role with the **opposite** orientation (a
/// publish is the bridge's *consumer* side). Deriving one from the other would be
/// a rename that reads as an inversion. **The identity — which is what
/// [FR-WS-27] is about — is resolved in exactly one place.**
///
/// A row whose enclosing declaration is unknown (its file was deleted, or the
/// symbol never bound) promotes nothing rather than hanging a producer off a
/// fabricated parent ([NFR-RA-05]).
///
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn broker_refs<'a>(
    refs: &'a [UnresolvedRefRow],
    node_by_symbol: &'a HashMap<&'a str, &'a NodeRow>,
    corpus: &dyn ConfigLookup,
) -> Vec<BrokerRef<'a>> {
    refs.iter()
        .filter_map(|row| {
            let relation = row.payload.as_deref().and_then(ArtifactRelation::from_wire)?;
            let kind = match relation {
                ArtifactRelation::BrokerPublish => NodeKind::Producer,
                ArtifactRelation::BrokerSubscribe => NodeKind::Consumer,
                _ => return None,
            };
            let identity = broker_identity::identify(relation, &row.target, corpus)?;
            let enclosing = *node_by_symbol.get(row.source_symbol.as_str())?;
            // **Flattened, one `BrokerRef` per topic identity.** A site whose
            // member commits its key two ways under two overlays promotes under
            // each of them, exactly as the bridge fans out under each
            // ([FR-WS-19] AC2) — never a value chosen between them. The
            // overwhelming case is one identity and one ref.
            Some(identity.topics.into_iter().map(move |topic| BrokerRef {
                kind,
                topic,
                enclosing,
                line: row.line,
            }))
        })
        .flatten()
        .collect()
}

/// The repo-scoped symbol of a topic: no path segments, so every member of the
/// repo that names `key` meets on **one** node ([FR-WS-11]).
fn topic_symbol(ctx: &SymbolContext, key: &str) -> Result<LogosSymbol> {
    crate::extract::symbol::build_symbol(
        ctx,
        &[],
        &[
            descriptor_for(NodeKind::Module, "topic", 0),
            descriptor_for(NodeKind::Topic, key, 0),
        ],
    )
}

/// The symbol of a publish/subscribe **site**: the topic descriptor hung off the
/// enclosing declaration's own symbol under a **role namespace**, so it is unique
/// per `(declaration, role, topic)` and lives in the file that declares it.
///
/// The role namespace (`producer/` | `consumer/`) is not decoration:
/// [`descriptor_for`] renders both roles as the same `name#` type descriptor, so
/// without it a **relay** — one declaration that subscribes to a topic and
/// re-publishes on it — would collide its producer and its consumer onto one symbol
/// and silently lose a real broker fact. Mirrors the framework pass's `route/` /
/// `component/` pseudo-namespace convention.
///
/// Load-bearing since migration 18 ([CR-080]): the ledger's uniqueness key now
/// includes the relation discriminator, so a relay's publish and subscribe rows
/// both reach this pass (asserted by
/// `a_relay_method_keeps_both_its_publish_and_subscribe_after_migration_18` in
/// `tests/broker_topic_promotion.rs`). This role namespace is what keeps the
/// relay's producer and consumer distinct once both arrive.
///
/// [CR-080]: ../../../docs/requests/CR-080-broker-relay-ledger-dedup.md
fn site_symbol(enclosing: &LogosSymbol, kind: NodeKind, key: &str) -> Result<LogosSymbol> {
    let role = match kind {
        NodeKind::Producer => "producer",
        _ => "consumer",
    };
    LogosSymbol::parse(&format!(
        "{}{}{}",
        enclosing.as_str(),
        descriptor_for(NodeKind::Module, role, 0),
        descriptor_for(kind, key, 0)
    ))
}

/// Assemble the desired promoted set from the broker ledger rows.
///
/// Deterministic ([NFR-RA-06]): rows are folded into a [`BTreeMap`] keyed by
/// symbol, and a repeated site (the same declaration publishing one topic twice)
/// collapses to one node carrying the **first** line — one producer of a topic per
/// declaration, never a duplicate per call.
///
/// Since [S-424] a single ledger row can contribute **several** entries, because
/// [`broker_refs`] flattens an overlay disagreement into one ref per committed
/// value. That does not weaken the determinism above: `identify` returns its
/// topics sorted and de-duplicated, and the fold is keyed by symbol either way, so
/// the output is a function of the ledger and the corpus and of nothing else —
/// ledger order included.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [S-424]: ../../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
fn desired_set(
    refs: &[UnresolvedRefRow],
    nodes: &[NodeRow],
    file_id_by_path: &HashMap<&str, i64>,
    corpus: &dyn ConfigLookup,
) -> BTreeMap<String, DesiredNode> {
    let ctx = SymbolContext::default();
    let node_by_symbol: HashMap<&str, &NodeRow> =
        nodes.iter().map(|n| (n.symbol.as_str(), n)).collect();

    let mut desired: BTreeMap<String, DesiredNode> = BTreeMap::new();

    for r in broker_refs(refs, &node_by_symbol, corpus) {
        let topic_key = r.topic.as_str();
        let Ok(topic) = topic_symbol(&ctx, topic_key) else {
            continue; // a key that cannot be encoded as a symbol is refused, not coerced
        };
        let Ok(site) = site_symbol(&r.enclosing.symbol, r.kind, topic_key) else {
            continue;
        };

        // The shared topic node. Repo-scoped: the first row to name it creates it,
        // every later row on the same key finds it.
        desired
            .entry(topic.as_str().to_string())
            .or_insert_with(|| DesiredNode {
                symbol: topic.clone(),
                kind: NodeKind::Topic,
                name: topic_key.to_string(),
                file_id: None,
                start_line: None,
                end_line: None,
                edges: Vec::new(),
            });

        // The site node, anchored under the declaration that publishes/subscribes.
        let file_id = r
            .enclosing
            .file_path
            .as_deref()
            .and_then(|p| file_id_by_path.get(p).copied());
        let broker_edge = match r.kind {
            NodeKind::Producer => DesiredEdge::Publishes(topic.as_str().to_string()),
            _ => DesiredEdge::Subscribes(topic.as_str().to_string()),
        };
        let entry = desired
            .entry(site.as_str().to_string())
            .or_insert_with(|| DesiredNode {
                symbol: site,
                kind: r.kind,
                name: topic_key.to_string(),
                file_id,
                start_line: r.line,
                end_line: r.line,
                edges: vec![DesiredEdge::ContainedBy(r.enclosing.id), broker_edge],
            });
        // A second capture of the same site keeps the earliest line — deterministic
        // regardless of ledger order. The fold must be **total**: `None` means "line
        // unknown", so a known line must win over it in either arrival order. (A
        // partial fold guarded on `(Some, Some)` would leave the node at `None` when
        // the unknown-line row happened to arrive first, making the output depend on
        // ledger order — exactly what [NFR-RA-06] forbids.)
        entry.start_line = match (entry.start_line, r.line) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, b) => b,
        };
        entry.end_line = entry.start_line;
    }

    desired
}

/// `true` for an edge kind this pass **owns** around its promoted nodes: the
/// `Contains` anchoring and the two broker edges. Every other kind incident to a
/// promoted node belongs to another pass and is left untouched, so this pass can
/// never delete an edge it did not create (the never-clobber companion of
/// never-fabricate, [NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn is_topic_owned(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Contains | EdgeKind::Publishes | EdgeKind::Subscribes
    )
}

/// Reconcile the graph's promoted broker nodes to `desired`: a thin adapter
/// over the shared [`promote::reconcile`] primitive (CR-082).
///
/// The broker edges name their target [`Topic`](NodeKind::Topic) by **symbol**
/// (see [`DesiredEdge`]) because it may be created in this very batch, so this
/// pass's `resolve_edge` looks the target up in the batch symbol→id map and
/// drops the edge rather than fabricating a node when it is absent
/// ([NFR-RA-05]). Ownership ([`is_topic_owned`]: `Contains`/`Publishes`/
/// `Subscribes`) scopes the edge-level reconciliation to this pass's own kinds.
fn commit(
    runtime: &Runtime,
    existing: &[&NodeRow],
    edges: &[crate::graph_store::EdgeRow],
    desired: BTreeMap<String, DesiredNode>,
) -> Result<()> {
    let desired: Vec<Promoted<DesiredEdge>> = desired
        .into_values()
        .map(|d| Promoted {
            symbol: d.symbol,
            kind: d.kind,
            name: d.name,
            file_id: d.file_id,
            start_line: d.start_line,
            end_line: d.end_line,
            edges: d.edges,
        })
        .collect();

    promote::reconcile(
        runtime,
        existing,
        edges,
        desired,
        is_topic_owned,
        |self_id, edge, ids| match edge {
            DesiredEdge::ContainedBy(parent) => {
                Some(PromotedEdge::new(*parent, self_id, EdgeKind::Contains))
            }
            DesiredEdge::Publishes(topic) => ids
                .get(topic.as_str())
                .map(|&t| PromotedEdge::new(self_id, t, EdgeKind::Publishes)),
            DesiredEdge::Subscribes(topic) => ids
                .get(topic.as_str())
                .map(|&t| PromotedEdge::new(self_id, t, EdgeKind::Subscribes)),
        },
    )
}

#[cfg(test)]
mod tests;
