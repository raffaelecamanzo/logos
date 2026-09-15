//! **S-404 — the sibling client-call arms' over-capture, measured per language**
//! ([CR-128] §6 criteria 1-2, [FR-WS-08] AC5, [FR-WS-05], [NFR-RA-05]).
//!
//! # The question
//!
//! [S-402]'s audit found that **six** of the ten arms declaring
//! `http_client_detectors` decide client-call candidacy on the enclosing FILE's
//! import ledger, with no constraint on the receiver. `go` is corrected; five
//! siblings remain — `rust`, `kotlin`, `ruby`, `php`, `c-sharp`. [CR-128] §6
//! makes measurement a **blocking gate** on porting Go's receiver rule to any
//! of them, on the explicit ground that Go's 26-of-37 came from an HTTP
//! *gateway* — the member shape that maximises the defect — and does not
//! generalise.
//!
//! This module is that gate. Per language it walks a named corpus, runs the
//! **real** compiled grammar, the **real** per-language `invocations.scm` and
//! the **real** `extract::extract` pass (for the [FR-FW-04] ledger gate),
//! enumerates the client-call sites the arm would consider, and classifies each
//! site's RECEIVER as HTTP-client-named or not.
//!
//! # Why it owns its corpus resolution, and does not reuse the parent's
//!
//! Every other measurement in this harness reads one `LOGOS_REF_WORKSPACE`,
//! because every other measurement is about one estate. This one is about five
//! languages, and no single workspace holds them: the reference estate is Java,
//! Go, PHP, Python and TypeScript, so **four** of the five languages at issue
//! (`rust`, `kotlin`, `ruby`, `c-sharp`) have zero files in it and the fifth
//! (`php`) has 161 that import no client. So each arm resolves its own corpus
//! from its own variable ([`Arm::env`]), and a language whose variable is unset
//! is reported [`Outcome::Unmeasured`] — never as a zero.
//!
//! # A corpus that admits no file has not measured the gate
//!
//! [CR-128]'s decision log states it as a rule: *"Reporting an absent corpus as
//! a zero is the exact reading that let this defect survive in Go behind a
//! passing fixture set."* The same reading fails one step later, and this
//! module names that step separately: a corpus can exist, be walked, and
//! contain **no file that imports the language's client package at all**. The
//! ledger gate then admits nothing, the arm considers nothing, and the run
//! produces a zero that says nothing whatever about the receiver rule.
//!
//! [`Outcome::GateUnexercised`] is that state, and it is distinct from
//! [`Outcome::Measured`] with a zero non-HTTP share. Only the latter closes a
//! language out. [`Outcome::is_clean`] is the predicate, and it is pinned by
//! fixtures that run with no corpus at all — see
//! [`neither_an_absent_corpus_nor_an_unexercised_gate_reads_as_clean`].
//!
//! # [S-402]'s trap, named in the figures
//!
//! [CR-128] §4.4 records it: *"a fixture whose receiver the new rule refuses
//! stops testing the ledger gate, because its positive control becomes
//! unreachable."* A gate-isolating fixture works by showing the same source
//! captures WITH the client import and not without it. If a later receiver rule
//! refuses that fixture's receiver, both halves go silent together and the
//! fixture passes while testing nothing.
//!
//! The trap bears on this module's own figures in one specific way, stated here
//! because a reader of the table cannot see it: **every non-HTTP-named site
//! counted below is a site a receiver rule would refuse**, and each one is a
//! site that some fixture somewhere may be using as a positive control. The
//! enumeration this module prints is therefore the input to a port, not only
//! its justification — a port reads it to find which receivers stay accepted,
//! and re-points its gate-isolating fixture at one of those.
//!
//! Rust's own pin below is written against that trap: its positive control is
//! the bare word `client`, the one receiver name every shipped receiver rule
//! (Java's, Go's) accepts WHOLE, so the pin keeps working after a port lands.
//!
//! # What is asserted, and what is only reported
//!
//! Asserted, with no corpus (eight of the nine tests here): the receiver
//! reduction — both as a text rule and end-to-end through each arm's real
//! grammar — and the segment split beneath it; the classifier's boundary rule
//! on both edges; each arm's plugin name and corpus variable; the
//! [`Outcome::is_clean`] predicate and the materiality floor the verdict reads;
//! and Rust's stated ceiling, as a gate-isolating pair.
//!
//! Reported without assertion: the per-language counts. They are a property of
//! the corpora, not of the code, and pinning a corpus figure in an assertion is
//! how a measurement becomes a thing to be made green.
//!
//! [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
//! [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-FW-04]: ../../docs/specs/requirements/FR-FW-04.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use logos_core::extract::{self, FileInput, SymbolContext};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;
use tree_sitter::{Node, Parser};

/// S-404's recorded verdict, reproduced by [`measure_the_sibling_client_call_gates`]
/// and printed by it.
///
/// `include_str!` rather than a doc link, following
/// [`super::RECORDED_REFUSAL_FINDING`]: a file the build embeds cannot be
/// deleted or renamed without breaking compilation, so the artifact and the run
/// that produced it cannot drift apart silently.
const RECORDED_GATE_FINDING: &str = include_str!("client_call_gate_finding.txt");

// ── The five arms ───────────────────────────────────────────────────────────

/// One of [CR-128]'s five remaining file-grained arms.
///
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Arm {
    Rust,
    Kotlin,
    Ruby,
    Php,
    CSharp,
}

impl Arm {
    /// In [CR-128] §3.1's table order, minus the corrected `go` row.
    const ALL: [Arm; 5] = [Arm::Rust, Arm::Kotlin, Arm::Ruby, Arm::Php, Arm::CSharp];

    /// The plugin name, as `LanguagePlugin::name` spells it — the key this
    /// module matches a walked file's plugin against.
    fn plugin_name(self) -> &'static str {
        match self {
            Arm::Rust => "rust",
            Arm::Kotlin => "kotlin",
            Arm::Ruby => "ruby",
            Arm::Php => "php",
            Arm::CSharp => "c-sharp",
        }
    }

    /// The environment variable naming this arm's corpus.
    ///
    /// One variable per language rather than one shared root: the five corpora
    /// are five unrelated trees on any machine that has them at all, and a
    /// single root would force the unmeasured languages to masquerade as
    /// zero-file members of whichever tree was configured.
    fn env(self) -> &'static str {
        match self {
            Arm::Rust => "LOGOS_CLIENT_CALL_CORPUS_RUST",
            Arm::Kotlin => "LOGOS_CLIENT_CALL_CORPUS_KOTLIN",
            Arm::Ruby => "LOGOS_CLIENT_CALL_CORPUS_RUBY",
            Arm::Php => "LOGOS_CLIENT_CALL_CORPUS_PHP",
            Arm::CSharp => "LOGOS_CLIENT_CALL_CORPUS_CSHARP",
        }
    }

    /// Receiver names admitted only as the **whole** name.
    ///
    /// Go's rule is the template and its sharpest line is adopted verbatim: the
    /// generic word `client` names every client protocol in existence, so
    /// `cacheClient`, `zkClient` and `clientRegistry` must not clear it. Whole
    /// only, never a token run.
    ///
    /// The package qualifier a language's import binds belongs here for the
    /// same reason — `http` is the whole of Ruby's `Net::HTTP.get`, and
    /// `httpStatus` is not a client.
    fn whole_names(self) -> &'static [&'static str] {
        match self {
            // `reqwest::Client::new()` binds `Client`; a `let client = …` is the
            // idiom FR-WS-08's Rust row ("`reqwest`-class receiver-method
            // calls") describes.
            Arm::Rust => &["client", "http"],
            // FR-WS-08's Kotlin row is "as Java (API-compatible)".
            Arm::Kotlin => &["client", "http"],
            // `Net::HTTP.get(uri)` reduces to the receiver `Net::HTTP`, whose
            // last segment is `http`.
            Arm::Ruby => &["client", "http", "faraday"],
            Arm::Php => &["client", "http"],
            Arm::CSharp => &["client", "http"],
        }
    }

    /// Receiver name TOKENS — matched against any contiguous run of the
    /// receiver's camel/snake segments, so `apiHttpClient` and `_http_client`
    /// clear `httpclient` while `clientCache` clears nothing.
    ///
    /// Each vocabulary is that language's own normative row in [FR-WS-08] §
    /// "per-language capture", never a cross-language union: the *shape* of the
    /// rule is shared, the words are not.
    ///
    /// [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
    fn token_names(self) -> &'static [&'static str] {
        match self {
            // "`reqwest`-class receiver-method calls", plus the other five
            // crates the descriptor's `http_client_detectors` names.
            Arm::Rust => &[
                "httpclient", "reqwest", "hyper", "isahc", "ureq", "awc", "surf",
            ],
            // "as Java (API-compatible)" — Spring's three plus the JDK's.
            //
            // `okhttp` was here and is removed: it appears in FR-WS-08's Kotlin
            // row, in `kotlin/plugin.toml`'s detectors and in Java's nowhere, so
            // it was an unsourced widening — and a widening ACCEPTS more
            // receivers, which shrinks the non-HTTP share this module reports
            // and argues against the very port it gates. (`okHttpClient` still
            // clears, via the `httpclient` token run.)
            Arm::Kotlin => &["httpclient", "restclient", "resttemplate", "webclient"],
            // `Net::HTTP`, Faraday.
            Arm::Ruby => &["httpclient", "nethttp", "faraday"],
            // Guzzle.
            Arm::Php => &["httpclient", "guzzle", "guzzleclient"],
            // `HttpClient`.
            Arm::CSharp => &["httpclient"],
        }
    }
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.plugin_name())
    }
}

// ── The receiver, and what makes one client-named ───────────────────────────

/// The separators a receiver-method callee can be spelled with across the five
/// grammars: PHP's `->` and `?->` (the latter ends in `->`), the path `::`,
/// Kotlin's and C#'s safe call `?.`, Ruby's safe navigation `&.`, and the plain
/// `.`.
///
/// # The cut is taken at the separator that ENDS last, longest wins a tie
///
/// Array order decides nothing — the selection in [`receiver_of`] is what has
/// to be right, and the naive spelling (`max` over the separator's START index)
/// is wrong for every separator that has a shorter one as a **suffix**. `.` is a
/// suffix of `?.` and of `&.`, and it starts one byte LATER, so a start-index
/// rule cut `httpClient?.get` at the `.` and produced the receiver
/// `"httpClient?"` — whose trailing `?` then defeated both the whole-name rule
/// and the token-run rule, classifying a genuine client as non-HTTP.
///
/// That mattered in the fail-unsafe direction: a safe-called client landing in
/// the non-HTTP column **inflates** the over-capture share [CR-128] §6 gates a
/// port on. It was invisible in the recorded run only because Rust has no safe
/// call and the three languages that do were all GATE-UNEXERCISED — it would
/// have fired on the first re-measurement the finding itself asks for.
///
/// Pinned by [`the_receiver_reduction_handles_each_arms_spelling`] over all
/// three spellings, and end-to-end through the real grammars by
/// [`the_receiver_reduction_runs_over_each_arms_real_grammar`].
///
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
const CALLEE_SEPARATORS: [&str; 6] = ["->", "::", "?.", "&.", ".", "?->"];

/// The subset of [`CALLEE_SEPARATORS`] that accesses a VALUE on another value,
/// as opposed to `::`, which qualifies one type path. [`receiver_parts`] splits
/// on these to find the last access and on `::` within it.
const VALUE_SEPARATORS: [&str; 5] = ["->", "?->", "?.", "&.", "."];

/// The cut for `callee`: the separator whose END offset is greatest, ties
/// broken toward the LONGEST separator.
///
/// One spelling, used by both [`receiver_of`] and [`receiver_parts`], because
/// those two disagreed about `?.` in this module's first draft — one keyed on
/// the start offset and the other on the end — and two readers over one notion
/// is the hand-mirrored-twin defect the rest of this harness is careful to
/// avoid. Returns `(start, end)` of the winning separator.
fn last_separator(callee: &str, separators: &[&str]) -> Option<(usize, usize)> {
    separators
        .iter()
        .filter_map(|sep| callee.rfind(sep).map(|i| (i, i + sep.len(), sep.len())))
        .max_by_key(|(_, end, len)| (*end, *len))
        .map(|(start, end, _)| (start, end))
}

/// The receiver expression of the call whose first argument is `arg`, as source
/// text, or `None` when the argument has no enclosing call node.
///
/// # Why this is read from the source text and not from a field name
///
/// The five grammars spell a receiver-method call five different ways —
/// `field_expression` under `call_expression` (Rust), `navigation_expression`
/// (Kotlin), a `receiver:`/`method:` field pair (Ruby), `member_call_expression`
/// and `scoped_call_expression` (PHP), `member_access_expression` under
/// `invocation_expression` (C#). A field-name reader would be five readers, and
/// five readers over one notion is exactly the hand-mirrored-twin defect this
/// harness's reuse discipline exists to avoid.
///
/// The text between the call node's start and its first argument is the callee
/// followed by its opening delimiter, in all five. That is what this reads.
///
/// **Stated limit:** a callee spanning a comment or a line break keeps that
/// whitespace, and a receiver that is itself a call
/// (`s.client().get("/p")`) reduces to the text `s.client()`, whose last
/// segment is then `client()`. Both are reported as they are rather than
/// normalised — the census below prints every distinct receiver, so a shape the
/// reduction handles badly is visible rather than silently binned.
fn receiver_of<'t>(arg: Node<'t>, src: &'t [u8]) -> Option<String> {
    let mut node = arg;
    let call = loop {
        let parent = node.parent()?;
        let kind = parent.kind();
        if kind.contains("call") || kind.contains("invocation") {
            break parent;
        }
        node = parent;
    };
    let start = call.start_byte();
    let end = arg.start_byte();
    if end <= start {
        return None;
    }
    let head = std::str::from_utf8(src.get(start..end)?).ok()?;
    // Strip the argument-list delimiter and anything after the callee: `(`, and
    // for a shape that reached the argument through a lambda or a leading
    // separator, whatever whitespace follows it.
    let callee = head.trim_end().trim_end_matches(['(', '[', '{', ' ', '\t', '\n', '\r']);
    let callee = callee.trim();
    if callee.is_empty() {
        return None;
    }
    // Everything before the final separator is the receiver; the segment after
    // it is the verb.
    let (cut, _) = last_separator(callee, &CALLEE_SEPARATORS)?;
    let receiver = strip_leading_keywords(callee[..cut].trim());
    if receiver.is_empty() {
        None
    } else {
        Some(receiver.to_string())
    }
}

/// Expression keywords a grammar can leave inside the receiver slice.
///
/// tree-sitter-c-sharp gives `await client?.GetAsync("/x")` an
/// `invocation_expression` spanning the whole **await** expression, so the
/// byte-slice rule above yields `await client` — and `segments` does not split
/// on a space, so the whole-name rule never saw `client` and a genuine
/// `HttpClient` call classified as non-HTTP. (The non-safe-called form is
/// unaffected: its `invocation_expression` starts at `client`. So the two
/// spellings of one call disagreed.)
///
/// Stripping the leading keyword run is preferred over treating whitespace as a
/// segment boundary: the latter would also make `client /* c */` classify as a
/// client, and a receiver carrying a comment is a shape this reduction has
/// always declared it does not normalise.
const RECEIVER_KEYWORDS: [&str; 4] = ["await", "return", "yield", "new"];

/// Drop any leading [`RECEIVER_KEYWORDS`] run from a receiver slice.
fn strip_leading_keywords(receiver: &str) -> &str {
    let mut out = receiver.trim();
    loop {
        let Some((head, rest)) = out.split_once(char::is_whitespace) else { return out };
        if !RECEIVER_KEYWORDS.contains(&head) {
            return out;
        }
        out = rest.trim_start();
    }
}

/// The names the boundary rule is applied to: the receiver's last **value**
/// access, split into its **path** segments, with the sigils PHP and Ruby write
/// on a variable stripped.
///
/// # Why the two separator classes are not the same separator
///
/// A `.` or `->` accesses a value on another value, so only the last one names
/// the receiver: in `client.headers()` the receiver of the verb is the header
/// map, not the client, and reading `client` there would classify a header
/// lookup as an outbound call — the exact fabrication [NFR-RA-05] forbids and
/// the one [S-402] found 26 instances of in Go.
///
/// A `::` qualifies a *type or module path*, and every segment of it names the
/// same thing: `reqwest::Client::new()` IS a `reqwest` client, and reading only
/// its last segment (`new()`) classified three genuine `reqwest` calls as
/// non-HTTP in this module's first run. Each `::` part is therefore tested.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
/// [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
fn receiver_parts(receiver: &str) -> Vec<String> {
    let value = last_separator(receiver, &VALUE_SEPARATORS)
        .map_or(receiver, |(_, end)| &receiver[end..]);
    value
        .split("::")
        .map(|p| p.trim().trim_start_matches(['$', '@', '&', '*']).to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// A name's camel / snake / digit segments, lower-cased.
///
/// `httpClient` → `["http", "client"]`, `_http_client2` → `["http", "client",
/// "2"]`, `HTTPClient` → `["http", "client"]`.
fn segments(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1).copied();
        let boundary = match prev {
            None => false,
            Some(p) => {
                // lower|digit → upper, or upper → upper followed by lower
                // (`HTTPClient` breaks before the `C`).
                (!p.is_uppercase() && c.is_uppercase())
                    || (p.is_uppercase()
                        && c.is_uppercase()
                        && next.is_some_and(|n| n.is_lowercase()))
                    || (p.is_alphabetic() && c.is_ascii_digit())
                    || (p.is_ascii_digit() && c.is_alphabetic())
            }
        };
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether a receiver name clears this arm's client vocabulary.
///
/// A **boundary** rule, never a substring test, in the shape Java ([S-375]) and
/// Go ([S-402]) both ship:
///
///   * the whole name equals one of [`Arm::whole_names`], or
///   * some contiguous run of its segments joins to one of
///     [`Arm::token_names`].
///
/// So `client` and `http_client` and `apiHttpClient` clear it; `clientCache`,
/// `clientele`, `notaReqwest` and `subscriber` do not.
///
/// # This is a proxy, and the census beside it is the evidence
///
/// The question [CR-128] §6 asks is whether a captured site is *an HTTP call*.
/// A name rule cannot answer that — it answers whether the receiver is
/// *spelled* like a client, which is the same rule a port would apply and
/// therefore the right predicate for deciding whether a port is worth filing.
/// The report prints every distinct receiver with its count beside the split,
/// so the classification can be checked by reading it rather than trusted.
///
/// [S-375]: ../../docs/planning/journal.md#s-375-the-client-call-detector-gate-is-receiver-grained-not-file-grained
/// [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
fn is_client_named(arm: Arm, receiver: &str) -> bool {
    receiver_parts(receiver).iter().any(|part| {
        let segs = segments(part);
        if segs.is_empty() {
            return false;
        }
        let whole = segs.join("");
        if arm.whole_names().iter().any(|w| *w == whole) {
            return true;
        }
        (0..segs.len()).any(|start| {
            (start + 1..=segs.len())
                .any(|end| arm.token_names().iter().any(|t| *t == segs[start..end].join("")))
        })
    })
}

// ── The per-language outcome ────────────────────────────────────────────────

/// One captured client-call site, as the report enumerates it.
#[derive(Debug, Clone)]
struct GateSite {
    file: String,
    line: u32,
    receiver: String,
    client_named: bool,
}

/// What a language's run produced. The three variants are **not**
/// interchangeable, and [`Outcome::is_clean`] is where that is enforced.
#[derive(Debug, Clone)]
enum Outcome {
    /// No corpus is configured for this language. [CR-128] §6 criterion 2: a
    /// missing corpus is reported unmeasured, never as a zero.
    ///
    /// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
    Unmeasured { reason: String },
    /// A corpus was walked, and **no file in it** passes the [FR-FW-04] ledger
    /// gate. The arm considered nothing, so the run says nothing about the
    /// receiver rule — a zero here is the absence of a question, not an answer
    /// to one.
    ///
    /// [FR-FW-04]: ../../docs/specs/requirements/FR-FW-04.md
    GateUnexercised { corpus: String, files: usize },
    /// A corpus was walked and the gate admitted at least one file. This is the
    /// only variant whose zero closes a language out.
    Measured {
        corpus: String,
        files: usize,
        gated: usize,
        sites: Vec<GateSite>,
        /// What the arm actually WROTE over the same corpus, read from the
        /// production `extract` pass rather than from this module's mirrored
        /// site walk: `(references, refusal rows)`.
        ///
        /// Reported because the two harms of a file-grained gate are different
        /// sizes and a site count alone conflates them. A non-HTTP site whose
        /// argument is an absolute-path literal becomes a **fabricated
        /// reference** ([NFR-RA-05]); one whose argument is anything else
        /// becomes a keyless **refusal row** (S-374), which inflates the
        /// `base-url-runtime` denominator every egress figure is computed over
        /// ([FR-WS-05]) without fabricating an edge. Both are the defect; only
        /// the first invents a cross-service link.
        ///
        /// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
        /// [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
        recorded: (usize, usize),
    },
}

/// The site count below which a measured arm's non-HTTP share is reported as
/// too thin to act on.
///
/// # This is a judgement about evidence, NOT an acceptance floor
///
/// No measurement produced it and none could: it is a statement about when a
/// proportion stops carrying information, not a prediction about the product.
/// The distinction matters here because this repository has been bitten by
/// collapsing it — a figure measured as *derivable* became an acceptance
/// criterion the product then failed. So nothing asserts this constant, no
/// requirement cites it, and it gates exactly one thing: the English sentence
/// [`Outcome::verdict`] prints.
///
/// Ten is chosen because Ruby's corpus produced **one** site, and "1 of 1
/// (100%)" and "118 of 293 (40%)" must not read as the same kind of result.
/// Anything from about five upward would serve; the value is not load-bearing
/// and a language sitting near it should be re-measured on a better corpus
/// rather than argued over.
const MATERIALITY_FLOOR: usize = 10;

/// Whether a site count is too small for its non-HTTP share to mean anything.
fn total_is_thin(sites: usize) -> bool {
    sites < MATERIALITY_FLOOR
}

impl Outcome {
    /// Whether this run **closes the language out** — that is, shows the
    /// file-grained gate promoting no non-HTTP-named site.
    ///
    /// Only a [`Outcome::Measured`] run can, and only one that actually saw
    /// sites. An unmeasured language and an unexercised gate are both `false`,
    /// which is the whole of [CR-128]'s "a missing corpus is not a zero"
    /// decision expressed as code rather than as prose in a report.
    ///
    /// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
    fn is_clean(&self) -> bool {
        match self {
            Outcome::Unmeasured { .. } | Outcome::GateUnexercised { .. } => false,
            Outcome::Measured { sites, .. } => {
                !sites.is_empty() && sites.iter().all(|s| s.client_named)
            }
        }
    }

    /// `(sites, non-HTTP-named sites)`, or `None` when nothing was measured.
    fn split(&self) -> Option<(usize, usize)> {
        match self {
            Outcome::Measured { sites, .. } => {
                Some((sites.len(), sites.iter().filter(|s| !s.client_named).count()))
            }
            _ => None,
        }
    }

    /// What [CR-128] §6's blocking gate decides for this arm — the sentence a
    /// port decision is read off, with [`Outcome::is_clean`] doing the deciding
    /// rather than a reader re-applying the rule from the table.
    ///
    /// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
    fn verdict(&self) -> String {
        match self {
            Outcome::Unmeasured { reason } => {
                format!("DOES NOT CLEAR — unmeasured ({reason}); a missing corpus is not a zero")
            }
            Outcome::GateUnexercised { files, .. } => format!(
                "DOES NOT CLEAR — {files} files walked, 0 admitted by the ledger \
                 gate; the arm was never asked the question, so this is not clean"
            ),
            Outcome::Measured { sites, .. } if sites.is_empty() => {
                "DOES NOT CLEAR — gated files, but no captured site; the receiver \
                 was never exercised"
                    .to_string()
            }
            // A clean run is subject to the floor exactly as a dirty one is.
            // "1 of 1 clean" and "0 of 293 clean" are not the same kind of
            // result, and CLOSES OUT is the terminal verdict — it is the one
            // that stops a language being looked at again. Ruby measured
            // exactly one site; had that site's receiver been client-named,
            // the un-floored arm would have closed Ruby out on a sample of one.
            Outcome::Measured { sites, .. } if self.is_clean() && total_is_thin(sites.len()) => {
                format!(
                    "DOES NOT CLEAR — every captured site is client-named, but too few \
                     sites ({} < {MATERIALITY_FLOOR}) to tell a rate from an accident",
                    sites.len(),
                )
            }
            _ if self.is_clean() => {
                "CLOSES OUT — every captured site is on a client-named receiver; \
                 no port is justified here"
                    .to_string()
            }
            Outcome::Measured { sites, .. } => {
                let non = sites.iter().filter(|s| !s.client_named).count();
                let total = sites.len();
                let head = if total_is_thin(total) { "DOES NOT CLEAR" } else { "CLEARS" };
                let tail = if total_is_thin(total) {
                    format!(
                        "too few sites ({total} < {MATERIALITY_FLOOR}) to tell a rate from \
                         an accident; re-measure on a corpus with real client code"
                    )
                } else {
                    "a receiver-rule port is justified".to_string()
                };
                format!(
                    "{head} — {non} of {total} captured sites on a receiver no client \
                     rule accepts; {tail}"
                )
            }
        }
    }

    /// The one-word status this arm's row carries in the report.
    fn status(&self) -> &'static str {
        match self {
            Outcome::Unmeasured { .. } => "UNMEASURED",
            Outcome::GateUnexercised { .. } => "GATE-UNEXERCISED",
            Outcome::Measured { .. } => "MEASURED",
        }
    }
}

// ── The walk ────────────────────────────────────────────────────────────────

/// The corpus configured for `arm`, or `None`.
///
/// A variable that is **set but does not resolve to a directory** panics rather
/// than reporting the language unmeasured, following [`super::corpus_root`]: a
/// typo'd path would otherwise read exactly like an honestly absent corpus, and
/// those are the two states this whole module exists to keep apart.
fn corpus_for(arm: Arm) -> Option<PathBuf> {
    let raw = std::env::var(arm.env()).ok();
    // A set-but-blank value is the same class of typo as a mis-spelled path
    // (`export VAR=`, an unexpanded shell variable), so it takes the same loud
    // path rather than the parent's `None`. Returning `None` here told an
    // operator who HAD set the variable to set it — the one place this module
    // broke its own rule that the two states must never be confusable.
    if let Some(raw) = &raw {
        assert!(
            !raw.trim().is_empty(),
            "{} is set but blank — refusing to report {arm} unmeasured when the \
             corpus is merely an empty value",
            arm.env(),
        );
    }
    raw?;
    super::corpus_from_var(
        arm.env(),
        &format!("refusing to report {arm} unmeasured when the corpus is merely mis-spelled"),
    )
}

/// Walk `root`, measuring only files whose plugin is `arm`'s.
fn measure_arm(arm: Arm, root: &Path) -> Outcome {
    // The registry is loaded from a SCRATCH directory, never from `root`.
    //
    // `LanguageRegistry::load` takes a *project root* and treats it as the
    // plugin-override root: a `<root>/.logos/plugins/<lang>/queries/
    // invocations.scm` shadows the shipped query without a rebuild (FR-PL-04,
    // FR-PL-05). Loading from the corpus therefore let the corpus supply the
    // very query being measured. Demonstrated: a shadow narrowing the Rust
    // receiver to `client` turned a 3-site/1-non-HTTP corpus into
    // `0 (0%) non-HTTP` and printed `CLOSES OUT — no port is justified here`,
    // exit 0, with nothing in the report indicating a different query ran —
    // fabricating the exact conclusion CR-128 §6's gate exists to stop being
    // reached carelessly.
    //
    // This is the same hazard the walker settings below already guard against
    // (a developer's `~/.gitignore_global` must not move a published figure),
    // through a different door. The measurement needs only the embedded
    // grammars, so it takes them the way `rust_client_call_rows` does.
    let scratch = tempfile::tempdir().expect("tempdir for the plugin registry");
    let registry = LanguageRegistry::load(scratch.path()).expect("plugin registry loads");
    let symbols = SymbolContext::default();
    let mut files = 0usize;
    let mut gated = 0usize;
    let mut sites = Vec::new();
    let (mut references, mut refusals) = (0usize, 0usize);

    // `parents(false)`, `git_global(false)`, `ignore(false)` — the parent's
    // walker settings, for its reason: a developer's `~/.gitignore_global` must
    // not quietly change a published measurement.
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Some(plugin) = registry.for_path(&rel) else {
            continue;
        };
        if plugin.name() != arm.plugin_name() {
            continue;
        }
        let Some(query) = plugin.query("invocations") else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        files += 1;

        let facts = extract::extract(&FileInput::new(&rel, &source), plugin, &symbols);
        // The parent's spelling of the FR-FW-04 ledger gate, not a copy of it —
        // this predicate fixes the client-arm denominator in three published
        // measurements now, and a hand-written twin that later diverges is the
        // exact defect the reuse discipline here exists to avoid.
        if !super::gate_admits(plugin, &facts) {
            continue;
        }
        gated += 1;

        // The arm's own recorded output, straight from the production pass —
        // not from the mirrored site walk below, so the figure is what
        // `workspace status` would count.
        for r in facts
            .refs
            .iter()
            .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
        {
            if r.target.trim().is_empty() {
                refusals += 1;
            } else {
                references += 1;
            }
        }

        let mut parser = Parser::new();
        if parser.set_language(plugin.language()).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        let src = source.as_bytes();
        // The parent's site collector, for the same reason: it applies the
        // arm's own verb gate (`invocation_methods`) and its own one-row-per-
        // call containment rule, so this counts the arm's corpus rather than a
        // grep's.
        for (line, arg) in super::collect_sites(
            query,
            tree.root_node(),
            src,
            &plugin.semantics().invocation_methods,
        ) {
            let receiver = receiver_of(arg, src).unwrap_or_default();
            let receiver = receiver.split_whitespace().collect::<Vec<_>>().join(" ");
            sites.push(GateSite {
                file: rel.clone(),
                line,
                client_named: !receiver.is_empty() && is_client_named(arm, &receiver),
                receiver,
            });
        }
    }

    if gated == 0 {
        Outcome::GateUnexercised { corpus: root.display().to_string(), files }
    } else {
        Outcome::Measured {
            corpus: root.display().to_string(),
            files,
            gated,
            sites,
            recorded: (references, refusals),
        }
    }
}

/// Every arm's outcome, in [`Arm::ALL`] order.
fn measure_all() -> Vec<(Arm, Outcome)> {
    Arm::ALL
        .iter()
        .map(|&arm| {
            let outcome = match corpus_for(arm) {
                Some(root) => measure_arm(arm, &root),
                None => Outcome::Unmeasured {
                    reason: format!("{} is unset", arm.env()),
                },
            };
            (arm, outcome)
        })
        .collect()
}

// ── The measurement ─────────────────────────────────────────────────────────

/// **S-404 acceptance: [CR-128] §6's blocking gate, run per language.**
///
/// Prints, per arm: the corpus, the files walked, the files the ledger gate
/// admitted, the captured client-call sites, and the non-HTTP-named share —
/// then the receiver census that is the evidence behind that share, then the
/// recorded finding.
///
/// It **skips** when no arm has a corpus configured, and reports each arm's
/// status independently otherwise: one configured language does not make the
/// other four measured.
///
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
#[test]
fn measure_the_sibling_client_call_gates() {
    let outcomes = measure_all();
    if outcomes.iter().all(|(_, o)| matches!(o, Outcome::Unmeasured { .. })) {
        eprintln!(
            "SKIPPED: set at least one of {} to run S-404's per-language gate.",
            Arm::ALL.map(|a| a.env()).join(", ")
        );
        return;
    }

    println!("\nS-404 — the five file-grained client-call arms, per language");
    println!("  grain: one captured client-call SITE (the arm's own verb gate and");
    println!("         one-row-per-call containment rule), inside a ledger-gated file\n");
    println!(
        "  {:<9} {:<17} {:>7} {:>6} {:>6} {:>9}  {:<9} corpus",
        "language", "status", "files", "gated", "sites", "non-HTTP", "refs/refu"
    );
    println!(
        "  (refs/refu = what the arm WROTE over the same corpus, from the production\n            pass: `http-client-call` references and keyless S-374 refusal rows.\n            A non-HTTP site with an absolute-path literal becomes a fabricated\n            REFERENCE; one with anything else becomes a REFUSAL row, inflating the\n            base-url-runtime denominator without inventing an edge.)"
    );
    for (arm, outcome) in &outcomes {
        let (files, corpus) = match outcome {
            Outcome::Unmeasured { reason } => (0, reason.clone()),
            Outcome::GateUnexercised { corpus, files } => (*files, corpus.clone()),
            Outcome::Measured { corpus, files, .. } => (*files, corpus.clone()),
        };
        let (gated, recorded) = match outcome {
            Outcome::Measured { gated, recorded, .. } => (*gated, Some(*recorded)),
            _ => (0, None),
        };
        let (sites, _) = outcome.split().unwrap_or((0, 0));
        let share = match outcome.split() {
            Some((0, _)) | None => "       n/a".to_string(),
            Some((s, n)) => format!("{n:>3} ({:>3.0}%)", 100.0 * n as f64 / s as f64),
        };
        let wrote = match recorded {
            Some((refs, refusals)) => format!("{refs:>4}/{refusals:<5}"),
            None => "    -/-    ".to_string(),
        };
        println!(
            "  {:<9} {:<17} {:>7} {:>6} {:>6} {share}  {wrote} {corpus}",
            arm.to_string(),
            outcome.status(),
            files,
            gated,
            sites,
        );
    }

    // CR-128 §6's gate, decided per arm rather than left to the reader. This is
    // the only place the port decision is stated, and `is_clean` is what makes
    // it — so the rule that an unmeasured or unexercised language is never
    // clean is applied by the same code the fixtures pin, not re-derived here.
    println!("\n  CR-128 §6 gate, per arm:");
    for (arm, outcome) in &outcomes {
        println!("    {arm:<9} {}", outcome.verdict());
    }

    // The census. The split above is a name rule; this is what it was applied
    // to, so the classification can be read rather than trusted.
    for (arm, outcome) in &outcomes {
        let Outcome::Measured { sites, .. } = outcome else { continue };
        if sites.is_empty() {
            continue;
        }
        let mut census: BTreeMap<(bool, String), usize> = BTreeMap::new();
        for s in sites {
            *census.entry((s.client_named, s.receiver.clone())).or_default() += 1;
        }
        println!("\n  {arm} — receiver census ({} sites)", sites.len());
        for ((client_named, receiver), count) in &census {
            println!(
                "    {:<5} {count:>4}  {receiver}",
                if *client_named { "HTTP" } else { "non" },
            );
        }
        // Up to ten worked examples from EACH column, so a reader can open one.
        // Both columns, not only the non-HTTP one: a false positive in the HTTP
        // column shrinks the reported share, and a report that prints only the
        // sites supporting its headline cannot be checked against itself.
        for (label, want) in [("non-HTTP-named", false), ("HTTP-named", true)] {
            println!("  {arm} — first {label} sites:");
            for s in sites.iter().filter(|s| s.client_named == want).take(10) {
                println!("    {}:{}  {}", s.file, s.line, s.receiver);
            }
        }
    }

    println!("\n--- recorded finding ---\n{RECORDED_GATE_FINDING}");

    // The one thing that must hold whatever the corpora contain: a configured
    // corpus must have produced a walk. A language reported MEASURED or
    // GATE-UNEXERCISED over zero files means the walker matched no file of that
    // language at all, which is a mis-pointed variable reading as a result.
    for (arm, outcome) in &outcomes {
        let files = match outcome {
            Outcome::Unmeasured { .. } => continue,
            Outcome::GateUnexercised { files, .. } | Outcome::Measured { files, .. } => *files,
        };
        assert!(
            files > 0,
            "{}={} walked ZERO {arm} files — the corpus is configured but holds \
             none of that language, which is a mis-pointed variable, not a \
             measurement. Point it at a tree containing {arm} source or unset it \
             so {arm} is reported UNMEASURED.",
            arm.env(),
            std::env::var(arm.env()).unwrap_or_default(),
        );
    }
}

// ── Fixtures: they run with no corpus, and they are what pins the module ────

/// [CR-128] §6 criterion 2, and its decision log's *"a missing tool is not a
/// clean tool"*, as an executable predicate rather than a rule a report writer
/// must remember.
///
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
#[test]
fn neither_an_absent_corpus_nor_an_unexercised_gate_reads_as_clean() {
    let unmeasured = Outcome::Unmeasured { reason: "unset".into() };
    let unexercised = Outcome::GateUnexercised { corpus: "/tmp/x".into(), files: 161 };
    let empty = Outcome::Measured {
        corpus: "/tmp/x".into(),
        files: 10,
        gated: 3,
        sites: Vec::new(),
        recorded: (0, 0),
    };
    // 12 sites: a literal above the floor, NOT `MATERIALITY_FLOOR` sites.
    // Deriving the fixture from the constant made the pair hold for every value
    // of the constant, so raising it to 1000 — which flips the real 293-site run
    // from CLEARS to "too few sites" — left the suite green.
    let clean = Outcome::Measured {
        corpus: "/tmp/x".into(),
        files: 10,
        gated: 3,
        sites: (0..12)
            .map(|i| GateSite {
                file: "a.rs".into(),
                line: i,
                receiver: "client".into(),
                client_named: true,
            })
            .collect(),
        recorded: (12, 0),
    };
    // The same shape below the floor: clean, and still not a closing-out result.
    let thin_clean = Outcome::Measured {
        corpus: "/tmp/x".into(),
        files: 10,
        gated: 3,
        sites: vec![GateSite {
            file: "a.rs".into(),
            line: 1,
            receiver: "client".into(),
            client_named: true,
        }],
        recorded: (1, 0),
    };
    let dirty = Outcome::Measured {
        corpus: "/tmp/x".into(),
        files: 10,
        gated: 3,
        sites: vec![GateSite {
            file: "a.rs".into(),
            line: 1,
            receiver: "cache".into(),
            client_named: false,
        }],
        recorded: (1, 0),
    };

    assert!(!unmeasured.is_clean(), "an unconfigured corpus must never close a language out");
    assert!(
        !unexercised.is_clean(),
        "a corpus whose ledger gate admitted no file has asked the arm nothing — \
         its zero is the absence of a question, not an answer"
    );
    assert!(
        !empty.is_clean(),
        "a gated corpus that yielded no captured site has not exercised the \
         receiver either"
    );
    assert!(clean.is_clean(), "every site client-named IS the closing-out result");
    assert!(!dirty.is_clean());
    assert_eq!(unmeasured.status(), "UNMEASURED");
    assert_eq!(unexercised.status(), "GATE-UNEXERCISED");
    // The verdict the report prints is decided by `is_clean`, so the rule and
    // the sentence a reader acts on cannot drift apart.
    for o in [&unmeasured, &unexercised, &empty, &dirty] {
        assert!(
            o.verdict().starts_with("DOES NOT CLEAR") || o.verdict().starts_with("CLEARS"),
            "no non-clean outcome may print a closing-out verdict: {}",
            o.verdict()
        );
        assert!(!o.verdict().starts_with("CLOSES OUT"));
    }
    assert!(clean.verdict().starts_with("CLOSES OUT"));
    // `dirty` carries ONE site, so it is below the materiality floor and must
    // not read as a port justification — the contradiction this catches is a
    // real one the first run of this module shipped, where the table said ruby
    // was too thin to act on and the verdict line said a port was justified.
    assert!(dirty.verdict().starts_with("DOES NOT CLEAR"), "{}", dirty.verdict());
    assert!(dirty.verdict().contains("too few sites"));
    // Again a literal, for the reason given at `clean` above.
    let material = Outcome::Measured {
        corpus: "/tmp/x".into(),
        files: 10,
        gated: 3,
        sites: (0..12)
            .map(|i| GateSite {
                file: "a.rs".into(),
                line: i,
                receiver: "cache".into(),
                client_named: false,
            })
            .collect(),
        recorded: (12, 0),
    };
    assert!(
        !total_is_thin(12) && total_is_thin(1),
        "these fixtures straddle the floor by construction; if MATERIALITY_FLOOR \
         has moved past 12, move the fixtures deliberately rather than deriving \
         them from the constant"
    );
    // A clean run below the floor does NOT close a language out.
    assert!(thin_clean.is_clean(), "it is clean by the predicate…");
    assert!(
        thin_clean.verdict().starts_with("DOES NOT CLEAR"),
        "…but one site cannot close a language out: {}",
        thin_clean.verdict()
    );
    assert!(thin_clean.verdict().contains("too few sites"));
    assert!(material.verdict().starts_with("CLEARS"), "{}", material.verdict());
    assert!(material.verdict().contains("port is justified"));
    // No verdict line may carry the run of spaces a mangled line continuation
    // leaves behind — the first run of this module printed five of them.
    for o in [&unmeasured, &unexercised, &empty, &clean, &dirty, &material, &thin_clean] {
        assert!(!o.verdict().contains("  "), "double space in: {}", o.verdict());
    }
    assert_eq!(dirty.split(), Some((1, 1)));
    assert_eq!(unexercised.split(), None, "an unexercised gate reports no share at all");
}

/// The classifier is a **boundary** rule on both edges — the near misses, not
/// the tidy cases.
///
/// Every shape here was picked because it sits one character from the other
/// side: `clientCache` is Go's own worked example of why the generic word is
/// whole-only, and `notaReqwest` is the substring test this must not be.
#[test]
fn the_receiver_rule_is_a_boundary_rule_never_a_substring_test() {
    // Accepted: the whole generic word, and the type-derived token in every
    // casing and affix the five languages write it in.
    for name in ["client", "http_client", "httpClient", "_httpClient", "apiHttpClient", "HTTPClient"]
    {
        assert!(is_client_named(Arm::Rust, name), "{name} must be client-named");
    }
    assert!(is_client_named(Arm::Rust, "reqwestClient"));
    assert!(is_client_named(Arm::Kotlin, "restTemplate"));
    assert!(is_client_named(Arm::Kotlin, "webClient"));
    assert!(is_client_named(Arm::Ruby, "faraday"));
    assert!(is_client_named(Arm::Ruby, "Net::HTTP"), "the receiver's LAST segment decides");
    assert!(is_client_named(Arm::Php, "$this->guzzleClient"));
    assert!(is_client_named(Arm::CSharp, "_httpClient"));

    // Refused: the generic word as a segment rather than the whole name, and
    // every near miss of a token.
    for name in ["clientCache", "cacheClient", "clientRegistry", "clientele", "clientcache"] {
        assert!(!is_client_named(Arm::Rust, name), "{name} must NOT be client-named");
    }
    // A vocabulary word that is not a whole SEGMENT is refused — the rule is a
    // token run, never a substring test.
    assert!(!is_client_named(Arm::Rust, "notareqwest"));
    assert!(!is_client_named(Arm::Rust, "reqwestless"));
    assert!(!is_client_named(Arm::Rust, "surfaces"), "`surf` is a segment of nothing here");
    // And the stated cost of that: a camel boundary DOES make it a segment, so
    // `notaReqwest` is admitted by exactly the rule that admits `apiHttpClient`.
    // A name rule cannot separate them and it is not worth a special case — the
    // census beside the split is what a reader checks this against.
    assert!(
        is_client_named(Arm::Rust, "notaReqwest"),
        "a camel boundary makes `Reqwest` a segment; this is the same admission \
         as `apiHttpClient` and is stated rather than special-cased"
    );
    assert!(!is_client_named(Arm::CSharp, "_cache"));
    assert!(
        !is_client_named(Arm::CSharp, "restTemplate"),
        "each arm carries ITS OWN normative vocabulary — Kotlin's words are not C#'s"
    );
    assert!(!is_client_named(Arm::Rust, "headers"));
    assert!(!is_client_named(Arm::Rust, ""));
}

/// `segments` is the split every vocabulary match is taken over, so the
/// acronym and digit edges are pinned here rather than inferred from the rule
/// above.
#[test]
fn segments_split_on_case_underscore_and_digit_boundaries() {
    assert_eq!(segments("httpClient"), ["http", "client"]);
    assert_eq!(segments("HTTPClient"), ["http", "client"]);
    assert_eq!(segments("_http_client"), ["http", "client"]);
    assert_eq!(segments("httpClient2"), ["http", "client", "2"]);
    // The digit -> alpha direction. `http2Client` does NOT reach it — the camel
    // boundary splits before the `C` first — so the case has to be the
    // all-lowercase one, which is how a snake-cased or lowercase name spells it.
    // Deleting the clause merges `2client` into one segment, and a genuine
    // client receiver is then refused.
    assert_eq!(segments("http2client"), ["http", "2", "client"]);
    assert_eq!(segments("oauth2client"), ["oauth", "2", "client"]);
    // Kept beside it so the two directions are visibly different cases.
    assert_eq!(segments("http2Client"), ["http", "2", "client"]);
    assert_eq!(segments("client"), ["client"]);
    assert!(segments("").is_empty());
    assert!(segments("__").is_empty());
}

/// Each arm's two identity strings resolve: the plugin name to a real plugin,
/// and the environment variable to this arm's own.
///
/// Neither had any coverage. `Arm::Rust => "kotlin"` in `plugin_name` left the
/// suite green while `measure_arm` walked Kotlin files and reported them under
/// the `rust` heading — `files > 0` still passes, because the other language's
/// files are there. That is the defect class this whole module exists to
/// prevent (a figure that is not what its label says), and it was the one
/// instance of it with no guard.
#[test]
fn each_arms_plugin_name_and_env_var_resolve() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    // One known extension per arm, resolved through the registry the walk uses,
    // so a plugin rename upstream fails here rather than silently re-labelling a
    // measurement.
    for (arm, ext) in [
        (Arm::Rust, "rs"),
        (Arm::Kotlin, "kt"),
        (Arm::Ruby, "rb"),
        (Arm::Php, "php"),
        (Arm::CSharp, "cs"),
    ] {
        let plugin = registry
            .for_extension(ext)
            .unwrap_or_else(|| panic!("{arm}: no plugin claims `.{ext}`"));
        assert_eq!(
            plugin.name(),
            arm.plugin_name(),
            "{arm}: `.{ext}` resolves to plugin `{}`, but this arm matches walked \
             files against `{}` — the walk would measure a different language \
             under this arm's heading",
            plugin.name(),
            arm.plugin_name(),
        );
        assert!(
            plugin.query("invocations").is_some(),
            "{arm}: the plugin ships no `invocations` query, so this arm can \
             never measure anything"
        );
    }
    // The env vars are distinct and each names its own arm — a copy-paste
    // collision would make two arms read one corpus.
    let vars: Vec<&str> = Arm::ALL.iter().map(|a| a.env()).collect();
    let unique: std::collections::BTreeSet<&str> = vars.iter().copied().collect();
    assert_eq!(unique.len(), Arm::ALL.len(), "the five corpus variables collide: {vars:?}");
    // Named independently here, not derived from `Arm::env`: a prefix check
    // passes for `LOGOS_CLIENT_CALL_CORPUS_TYPO` too, so it caught nothing. The
    // second spelling is the whole point — the variable an operator is told to
    // set has to be the one this arm reads.
    for (arm, var) in [
        (Arm::Rust, "LOGOS_CLIENT_CALL_CORPUS_RUST"),
        (Arm::Kotlin, "LOGOS_CLIENT_CALL_CORPUS_KOTLIN"),
        (Arm::Ruby, "LOGOS_CLIENT_CALL_CORPUS_RUBY"),
        (Arm::Php, "LOGOS_CLIENT_CALL_CORPUS_PHP"),
        (Arm::CSharp, "LOGOS_CLIENT_CALL_CORPUS_CSHARP"),
    ] {
        assert_eq!(arm.env(), var, "{arm} reads the wrong corpus variable");
    }
}

/// `receiver_of` reduces the five grammars' receiver-method spellings through
/// one text rule; `receiver_parts` takes the last VALUE access and splits it on
/// the PATH separator.
///
/// The last two cases are the pair that motivates the split, and they pull in
/// opposite directions: `client.headers()` must NOT read as a client (the
/// receiver of the verb is the header map), while `reqwest::Client::new()` must
/// (every `::` segment names the same type). A single "last segment" rule gets
/// one of them wrong whichever way it is spelled — the first run of this module
/// got the second one wrong, classifying three genuine `reqwest` calls non-HTTP.
#[test]
fn the_receiver_reduction_handles_each_arms_spelling() {
    assert_eq!(receiver_parts("cache"), ["cache"]);
    assert_eq!(receiver_parts("$this->client"), ["client"]);
    assert_eq!(receiver_parts("self.http_client"), ["http_client"]);
    assert_eq!(receiver_parts("Net::HTTP"), ["Net", "HTTP"]);
    assert_eq!(receiver_parts("@conn"), ["conn"]);
    assert_eq!(receiver_parts("_client"), ["_client"], "an underscore PREFIX is part of the name");

    assert_eq!(receiver_parts("client.headers()"), ["headers()"]);
    assert!(
        !is_client_named(Arm::Rust, "client.headers()"),
        "the receiver of the verb is the header map, not the client that \
         produced it — reading the prefix here is how a header lookup becomes a \
         fabricated outbound call"
    );
    assert_eq!(receiver_parts("reqwest::Client::new()"), ["reqwest", "Client", "new()"]);
    assert!(
        is_client_named(Arm::Rust, "reqwest::Client::new()"),
        "a `::` path names ONE thing, so every segment of it decides"
    );

    // Safe call / safe navigation. `.` is a SUFFIX of `?.` and `&.` and starts
    // one byte later, so a cut taken at the separator's START index picks the
    // `.` and leaves the `?`/`&` dangling on the receiver — which then defeats
    // both the whole-name and the token-run rule. Three of the five arms spell
    // their idiomatic call this way.
    assert_eq!(receiver_parts("httpClient?.get"), ["get"]);
    for (arm, receiver) in [
        (Arm::Kotlin, "httpClient?"),
        (Arm::Kotlin, "restClient?"),
        (Arm::CSharp, "_httpClient?"),
        (Arm::Ruby, "@http_client&"),
    ] {
        assert!(
            !is_client_named(arm, receiver),
            "a dangling safe-call sigil must not be part of the name: {receiver}"
        );
    }
}

/// The receiver reduction, end to end through each arm's **real** grammar and
/// **real** `invocations.scm` — the half
/// [`the_receiver_reduction_handles_each_arms_spelling`] does not reach.
///
/// [`receiver_of`] had no corpus-free test at all until this one: it was
/// exercised only through the env-gated corpus walk, so replacing its body with
/// `return None` left the whole suite green while every site in a real
/// measurement classified non-HTTP — the module's headline conclusion,
/// manufactured from a function that returns nothing.
///
/// Most cases below are a **pair**: the plain spelling and the safe-called one,
/// so the operator is provably the only difference between them.
#[test]
fn the_receiver_reduction_runs_over_each_arms_real_grammar() {
    fn receivers(ext: &str, source: &str) -> Vec<String> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
        let plugin = registry.for_extension(ext).unwrap_or_else(|| panic!("no {ext} grammar"));
        let query = plugin.query("invocations").expect("invocations query");
        let mut parser = Parser::new();
        parser.set_language(plugin.language()).expect("language");
        let tree = parser.parse(source, None).expect("parse");
        let src = source.as_bytes();
        super::collect_sites(query, tree.root_node(), src, &plugin.semantics().invocation_methods)
            .into_iter()
            .map(|(_, arg)| receiver_of(arg, src).unwrap_or_default())
            .collect()
    }

    // Rust: a plain receiver, and the `::` path whose LAST segment is `new()`.
    assert_eq!(
        receivers("rs", "use reqwest::Client;\nfn f(c: &Client) { let _ = c.get(\"/a\"); }\n"),
        vec!["c".to_string()],
    );
    assert_eq!(
        receivers(
            "rs",
            "use reqwest::Client;\nfn f() { let _ = reqwest::Client::new().get(\"/a\"); }\n",
        ),
        vec!["reqwest::Client::new()".to_string()],
    );

    // Kotlin: `?.` must reduce exactly as `.` does.
    let kt_plain = receivers("kt", "class A(val httpClient: C) { fun f() { httpClient.get(\"/a\") } }\n");
    let kt_safe = receivers("kt", "class A(val httpClient: C?) { fun f() { httpClient?.get(\"/a\") } }\n");
    assert_eq!(
        kt_plain, kt_safe,
        "a safe call must reduce to the same receiver as the plain call"
    );
    assert!(!kt_plain.is_empty(), "the kotlin fixture captured no site at all");
    assert!(
        kt_plain.iter().all(|r| is_client_named(Arm::Kotlin, r)),
        "kotlin receivers: {kt_plain:?}"
    );

    // C#: the safe-called form additionally hands back an `invocation_expression`
    // spanning the whole `await` expression, so the keyword strip is what makes
    // the two spellings agree.
    let cs_plain = receivers(
        "cs",
        "using System.Net.Http;\nclass A { HttpClient client; async void M() { await client.GetAsync(\"/x\"); } }\n",
    );
    let cs_safe = receivers(
        "cs",
        "using System.Net.Http;\nclass A { HttpClient client; async void M() { await client?.GetAsync(\"/x\"); } }\n",
    );
    assert_eq!(cs_plain, cs_safe, "`await` and `?.` must not change the receiver");
    assert!(!cs_plain.is_empty(), "the c# fixture captured no site at all");
    assert!(
        cs_plain.iter().all(|r| is_client_named(Arm::CSharp, r)),
        "c# receivers: {cs_plain:?}"
    );

    // Ruby: `&.` is safe navigation and is a separator like `.`.
    let rb_plain = receivers(
        "rb",
        "require 'net/http'\nclass A\n  def f; @http_client.get(\"/a\"); end\nend\n",
    );
    let rb_safe = receivers(
        "rb",
        "require 'net/http'\nclass A\n  def f; @http_client&.get(\"/a\"); end\nend\n",
    );
    assert_eq!(rb_plain, rb_safe, "safe navigation must reduce like a plain call");
    assert!(!rb_plain.is_empty(), "the ruby fixture captured no site at all");
    assert!(
        rb_plain.iter().all(|r| is_client_named(Arm::Ruby, r)),
        "ruby receivers: {rb_plain:?}"
    );

    // PHP: `->` and `?->` both END in `->`, so both already cut correctly. Pinned
    // so a later change to the separator set cannot quietly break them.
    let php = receivers(
        "php",
        "<?php\nuse GuzzleHttp\\Client;\nclass A { function f() { $this->client->get(\"/a\"); } }\n",
    );
    assert!(!php.is_empty(), "the php fixture captured no site at all");
    assert!(php.iter().all(|r| is_client_named(Arm::Php, r)), "php receivers: {php:?}");

    // The shape the reduction declares it does NOT normalise, pinned as the
    // stated limit it is rather than left as prose.
    assert_eq!(
        receivers("rs", "use reqwest::Client;\nfn f(s: &S) { let _ = s.client().get(\"/a\"); }\n"),
        vec!["s.client()".to_string()],
        "a receiver that is itself a call reduces to its own text, per the stated limit"
    );
}

// ── Rust's stated ceiling ───────────────────────────────────────────────────

/// Parse `source` as a Rust compilation unit through the **production**
/// extract pass, and return the `HttpClientCall` reference targets it emitted.
fn rust_client_call_rows(source: &str) -> Vec<String> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = registry.for_extension("rs").expect("rust grammar");
    let facts = extract::extract(
        &FileInput::new("client.rs", source),
        plugin,
        &SymbolContext::default(),
    );
    facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
        .map(|r| r.target.clone())
        .collect()
}

/// The arm's captured **references** — every row that named a route.
///
/// Left separate from [`rust_client_call_rows`] for the reason
/// `go_invocations.rs` states: since S-374 a declined site also writes a
/// **keyless** row, so a helper that filtered them away would let the refusal
/// path regress unnoticed. The ceiling below reads both populations, because
/// the file-grained gate harms both.
fn rust_client_call_targets(source: &str) -> Vec<String> {
    rust_client_call_rows(source).into_iter().filter(|t| !t.is_empty()).collect()
}

/// The number of keyless refusal rows the arm recorded (S-374).
fn rust_client_call_refusals(source: &str) -> usize {
    rust_client_call_rows(source).iter().filter(|t| t.is_empty()).count()
}

/// **[CR-128] §6's sixth criterion: `rust`'s file-grained over-capture, pinned
/// as a stated ceiling — regardless of what the measurement above found.**
///
/// `rust` is the one arm of the six with no pin at all. The other four state
/// the over-capture as a ceiling and assert it in a test
/// (`a_route_shaped_collection_get_inside_a_client_file_is_a_stated_ceiling`
/// for Kotlin and Ruby, `…_property_get_…` for PHP,
/// `a_bare_verb_method_call_inside_a_client_file_is_not_captured` for C#);
/// Rust — the arm the rest were ported FROM — states only its free-function
/// ceiling, so its silence propagated. An unpinned ceiling is prose, and prose
/// cannot fail.
///
/// What this pins is **today's behaviour**, not desired behaviour: a
/// `router.get("/config/features")` inside a file importing `reqwest` IS
/// captured, and that is an [ADR-54] over-capture ceiling recorded rather than
/// worked around. A port that narrows Rust to a receiver rule must change this
/// test, and changing it is the point — it is what makes the narrowing visible.
///
/// # [S-402]'s trap, and why the positive control is the bare word `client`
///
/// [CR-128] §4.4: *"a fixture whose receiver the new rule refuses stops testing
/// the ledger gate, because its positive control becomes unreachable."* The
/// control below is `client.get(…)` — the bare generic word, WHOLE, which is the
/// one receiver spelling both shipped receiver rules (Java's [S-375], Go's
/// [S-402]) accept. So when a port lands, this fixture's control survives it and
/// the ceiling half is what moves, which is the correct and visible outcome.
///
/// [CR-128]: ../../docs/requests/CR-128-client-call-candidacy-gate-siblings-are-file-grained.md
/// [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
/// [S-375]: ../../docs/planning/journal.md#s-375-the-client-call-detector-gate-is-receiver-grained-not-file-grained
/// [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
#[test]
fn a_non_client_receiver_call_inside_a_reqwest_file_is_a_stated_ceiling() {
    // The shapes are the ones the S-404 census actually found, not tidy
    // invented ones (CR-128 §6: "fixtures pinning the shapes the measurement
    // actually found"). `headers.get(…)` is the single largest bucket — 54 of
    // the 99 unambiguously non-HTTP rust sites — and `router.get(…)` on a
    // `HashMap` is S-402's `mapping.Get("authorize.claims")` in another
    // language, found at `matchit-0.7.3/examples/hyper.rs:38`.
    const SOURCE: &str = r#"use reqwest::Client;

pub async fn authorize(headers: &HeaderMap, router: &Router, client: &Client) {
    let _ = headers.get("content-type");
    let _ = router.get("/config/features");
    let _ = client.get("/api/orders");
}
"#;
    let targets = rust_client_call_targets(SOURCE);
    // `client.get` is the positive control: it proves the file WAS scanned, so
    // `router.get` appearing beside it is the ledger gate admitting a
    // non-client receiver — not the whole fixture quietly falling outside the
    // gate.
    assert_eq!(
        targets,
        // `dedup_sort_refs` orders the ledger by target, not by line.
        vec!["GET /api/orders".to_string(), "GET /config/features".to_string()],
        "Rust's gate is file-grained today: a `HashMap` lookup inside a \
         `reqwest` file is promoted to a cross-service REFERENCE alongside the \
         genuine call, because its argument happens to be an absolute-path \
         literal. This is the ADR-54 ceiling CR-128 §6 requires pinned. If a \
         receiver rule has landed for Rust, change this test deliberately and \
         record the site-count delta as a precision correction — do not delete it."
    );
    // The second, larger harm, pinned separately because it is a different
    // harm: a non-HTTP site whose argument is NOT an absolute path fabricates
    // no edge, but writes a keyless `base-url-runtime` row that inflates the
    // denominator every egress figure is computed over (FR-WS-05). The header
    // lookup is that shape, and it is the census's biggest bucket.
    assert_eq!(
        rust_client_call_refusals(SOURCE),
        1,
        "`headers.get(\"content-type\")` is not an outbound call and names no \
         route, so it writes a keyless refusal row rather than a reference — an \
         inflated denominator rather than a fabricated edge, and still the \
         file-grained gate's doing"
    );
}

/// The other half of the gate-isolating pair: the **same** source with no
/// client import captures nothing, so the ledger gate is what separates the two
/// cases and the ceiling above is scoped to a genuine client file.
///
/// Kept beside the ceiling rather than folded into it: [S-402]'s trap is that
/// these two halves go silent **together** when a receiver rule refuses the
/// fixture's receiver. Two tests make that visible as one failing and one
/// passing; one test would hide it.
///
/// [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained
#[test]
fn a_route_shaped_get_outside_a_reqwest_file_is_not_captured() {
    // Byte-for-byte the ceiling fixture's body, minus the one `use` line, so
    // the ledger gate is provably the ONLY difference between the two cases.
    const SOURCE: &str = r#"pub async fn authorize(headers: &HeaderMap, router: &Router, client: &Client) {
    let _ = headers.get("content-type");
    let _ = router.get("/config/features");
    let _ = client.get("/api/orders");
}
"#;
    assert!(
        rust_client_call_rows(SOURCE).is_empty(),
        "with no `reqwest`-class import the ledger gate closes on the whole \
         file and nothing is scanned — neither a reference nor a refusal row, \
         got {:?}",
        rust_client_call_rows(SOURCE)
    );
}
