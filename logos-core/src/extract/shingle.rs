//! Winnowed near-clone shingle fingerprints over normalized token streams
//! (CR-005, [FR-EX-09], [ADR-21]).
//!
//! A function's *shingle set* is a compact, rename-invariant fingerprint of its
//! body shape that the annotation engine clusters into near-clone groups
//! ([FR-AN-06], S-043) and the Uniqueness dimension consumes ([FR-QM-13],
//! S-044). It is **distinct** from the exact AST-shape fingerprint ([FR-AN-02],
//! [`super::shape`]): that one hash answers "is this byte-for-byte the same
//! shape?", while a shingle *set* supports the *Jaccard similarity* a near-clone
//! match needs.
//!
//! # The pipeline
//!
//! 1. **Normalize** the token stream of the declaration's subtree
//!    ([`normalized_tokens`]): identifier-class leaves collapse to one
//!    [`ID_PLACEHOLDER`] atom and other named literal leaves to
//!    [`LIT_PLACEHOLDER`], so two functions identical modulo identifier and
//!    literal *names* produce the same stream ([FR-EX-09] "identifiers and
//!    literals normalized to placeholder classes"). Operators, keywords, and
//!    punctuation keep their token kind (structure), and comments are dropped
//!    (whitespace is never a node).
//! 2. **k-gram + hash**: every contiguous run of [`K_GRAM`] tokens is hashed
//!    with a platform-independent [`blake3`] reduction ([`shingle_hash`]) to a
//!    `u64`, so the fingerprints are byte-identical across the four release
//!    targets ([NFR-RA-06], [ADR-17]).
//! 3. **Winnow** the k-gram hash sequence with window [`WINDOW`]
//!    ([`winnow`], Schleimer–Wilkerson–Aiken): a deterministic, position-robust
//!    subset that keeps the set small while guaranteeing a shared substring of
//!    sufficient length is detected.
//!
//! A function below the [`K_GRAM`]-token floor produces no shingles
//! ([FR-EX-09]) — there is no k-gram to hash, and a trivial body is not a clone
//! signal. The winnowing parameters are fixed, documented constants
//! ([FR-EX-09]), tuned so a small one-line edit perturbs only a bounded number
//! of shingles.
//!
//! # Determinism ([NFR-RA-06], [NFR-PE-02])
//!
//! Pure function of the parse tree: one pre-order leaf walk (the same single
//! pass cost class as [`super::complexity`]/[`super::shape`], holding the index
//! budget [NFR-PE-02]/[NFR-PE-03]), a fixed hash, and a deterministic winnow.
//! The returned vector is sorted and deduplicated, so the shingle *set* — what
//! Jaccard similarity reads — has one canonical byte representation.
//!
//! [FR-EX-09]: ../../../docs/specs/requirements/FR-EX-09.md
//! [FR-AN-02]: ../../../docs/specs/requirements/FR-AN-02.md
//! [FR-AN-06]: ../../../docs/specs/requirements/FR-AN-06.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
//! [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
//! [ADR-17]: ../../../docs/specs/architecture/decisions/ADR-17.md
//! [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md

use tree_sitter::Node;

/// The k-gram size: how many consecutive normalized tokens form one shingle.
/// A documented, fixed constant ([FR-EX-09]); five balances sensitivity (short
/// enough to share across a one-line edit) against specificity (long enough that
/// an incidental run is not a false clone signal).
pub(crate) const K_GRAM: usize = 5;

/// The winnowing window: the number of consecutive k-gram hashes one fingerprint
/// is selected from. A documented, fixed constant ([FR-EX-09]); the guarantee is
/// that any shared run of at least `K_GRAM + WINDOW - 1` tokens contributes a
/// common shingle.
pub(crate) const WINDOW: usize = 4;

/// The placeholder atom every identifier-class token collapses to, so renaming
/// a variable/parameter/field/type never changes the stream ([FR-EX-09]).
const ID_PLACEHOLDER: &str = "\u{1}id";

/// The placeholder atom every other named literal leaf collapses to, so a
/// changed constant value never changes the stream ([FR-EX-09]).
const LIT_PLACEHOLDER: &str = "\u{1}lit";

/// The byte separating tokens inside a k-gram before hashing — a control char
/// that cannot occur in a tree-sitter token kind, so distinct token boundaries
/// can never collide into one ambiguous string.
const TOKEN_SEP: u8 = 0x1f;

/// The winnowed shingle-hash set of `node`'s **body** ([FR-EX-09]).
///
/// `node` is a `Function`/`Method` declaration; its body is the `body`-field
/// subtree (the block every v1 grammar names `body`), so the signature — name,
/// parameters, return type — never enters the fingerprint and two functions
/// differing only in signature can still be near-clones by body. A declaration
/// with no `body` field (an abstract/interface method) or a body below the
/// [`K_GRAM`]-token floor yields no shingles. Returns the sorted, deduplicated
/// fingerprint hashes, deterministic and platform-independent ([NFR-RA-06]).
pub(crate) fn shingles(node: Node<'_>) -> Vec<u64> {
    let Some(body) = node.child_by_field_name("body") else {
        return Vec::new(); // no body (e.g. an abstract method) — no clone signal
    };
    let tokens = normalized_tokens(body);
    if tokens.len() < K_GRAM {
        return Vec::new(); // below the floor — no k-gram, no clone signal
    }
    let hashes: Vec<u64> = tokens.windows(K_GRAM).map(shingle_hash).collect();
    let mut fingerprints = winnow(&hashes, WINDOW);
    // Set semantics (Jaccard reads a set) + one canonical byte representation
    // for a byte-stable NodeFact (NFR-RA-06).
    fingerprints.sort_unstable();
    fingerprints.dedup();
    fingerprints
}

/// The normalized token stream of `node`'s subtree, in source (pre-order) order.
///
/// Leaf tokens only (a tree-sitter leaf has no children): identifier-class
/// leaves → [`ID_PLACEHOLDER`], other named literal leaves → [`LIT_PLACEHOLDER`],
/// comments dropped, anonymous tokens (operators/keywords/punctuation) kept as
/// their kind. Iterative on an explicit stack — never native recursion — for the
/// same input-controlled-depth safety as the sibling walks ([FR-IX-04]).
///
/// [FR-IX-04]: ../../../docs/specs/requirements/FR-IX-04.md
fn normalized_tokens(node: Node<'_>) -> Vec<&'static str> {
    let mut tokens: Vec<&'static str> = Vec::new();
    // Children pushed in reverse so they pop in source order — the token stream
    // must follow the source, or a clone and its twin could disagree on order.
    let mut stack: Vec<Node<'_>> = vec![node];
    while let Some(current) = stack.pop() {
        let kind = current.kind();
        // Error-recovery and zero-width missing nodes are dropped wholesale, the
        // same way comments are: a syntax error already marks the file partial
        // ([FR-IX-04]), and hashing an `ERROR` leaf as a literal would let two
        // unrelated malformed bodies share a spurious shingle. Skipping the whole
        // subtree keeps a partial-parse body's fingerprint a function of its
        // well-formed tokens only.
        if current.is_error() || current.is_missing() {
            continue;
        }
        // Comments vanish from the stream (FR-EX-09); covers `line_comment`,
        // `block_comment`, and other grammars' `comment` variants.
        if kind.ends_with("comment") {
            continue;
        }
        let mut cursor = current.walk();
        let children: Vec<Node<'_>> = current.children(&mut cursor).collect();
        if children.is_empty() {
            // A leaf token contributes one normalized atom.
            tokens.push(classify(current));
            continue;
        }
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    tokens
}

/// Map one leaf token to its normalized atom: identifiers and literals collapse
/// to placeholder classes; every other (anonymous) token keeps its kind, which
/// for tree-sitter *is* the literal token text (`+`, `return`, `{`) and is a
/// genuinely `'static` reference into the grammar's symbol table — structure
/// that distinguishes `a + b` from `a - b`.
fn classify(leaf: Node<'_>) -> &'static str {
    let kind: &'static str = leaf.kind();
    if leaf.is_named() {
        if kind.ends_with("identifier") {
            return ID_PLACEHOLDER; // a name — renaming it must not change the shape
        }
        return LIT_PLACEHOLDER; // a literal/content leaf — value-independent
    }
    kind // an anonymous operator/keyword/punctuation token — kept as structure
}

/// Hash one k-gram (a window of [`K_GRAM`] normalized tokens) to a `u64`.
///
/// Platform-independent ([NFR-RA-06]): a [`blake3`] hash over the tokens joined
/// by [`TOKEN_SEP`], reduced to the first eight digest bytes read big-endian, so
/// the value is byte-identical on every release target.
fn shingle_hash(gram: &[&str]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for (i, tok) in gram.iter().enumerate() {
        if i > 0 {
            hasher.update(&[TOKEN_SEP]);
        }
        hasher.update(tok.as_bytes());
    }
    let digest = hasher.finalize();
    let bytes = digest.as_bytes();
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// Winnowing ([Schleimer–Wilkerson–Aiken]): select fingerprints from the k-gram
/// hash sequence by taking, in every window of `w` consecutive hashes, the
/// minimum (rightmost on a tie), recording a new fingerprint only when the
/// selected position moves.
///
/// Deterministic and order-stable. A sequence shorter than `w` is one window. A
/// `w` of `0` is treated as `1` (defence-in-depth — the constant is never `0`).
///
/// [Schleimer–Wilkerson–Aiken]: https://doi.org/10.1145/872757.872770
fn winnow(hashes: &[u64], w: usize) -> Vec<u64> {
    if hashes.is_empty() {
        return Vec::new();
    }
    let w = w.max(1);
    let n = hashes.len();
    // Rightmost minimum of the window `[start, end)`.
    let rightmost_min = |start: usize, end: usize| -> usize {
        let mut m = start;
        for j in (start + 1)..end {
            if hashes[j] <= hashes[m] {
                m = j;
            }
        }
        m
    };
    if n <= w {
        return vec![hashes[rightmost_min(0, n)]];
    }
    let mut out = Vec::new();
    let mut last_pos = usize::MAX;
    for start in 0..=(n - w) {
        let m = rightmost_min(start, start + w);
        if m != last_pos {
            out.push(hashes[m]);
            last_pos = m;
        }
    }
    out
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tree_sitter::Parser;

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(src, None).unwrap()
    }

    fn first_function(tree: &tree_sitter::Tree) -> Node<'_> {
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "function_item" {
                return n;
            }
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no function_item in tree");
    }

    fn shingles_of(src: &str) -> Vec<u64> {
        let tree = parse(src);
        shingles(first_function(&tree))
    }

    /// The winnow primitive picks the rightmost minimum per window and records a
    /// fingerprint only when the chosen position moves.
    #[test]
    fn winnow_selects_window_minima() {
        // Windows of size 2 over [3,1,4,1,5]: mins at positions 1,2(=4? no),…
        // [3,1]→pos1(1); [1,4]→pos1(1) same; [4,1]→pos3(1); [1,5]→pos3 same.
        assert_eq!(winnow(&[3, 1, 4, 1, 5], 2), vec![1, 1]);
        // A sequence shorter than the window is a single window → one min.
        assert_eq!(winnow(&[9, 2, 7], 5), vec![2]);
        assert_eq!(winnow(&[], 4), Vec::<u64>::new());
    }

    /// FR-EX-09: two functions identical modulo identifier renames share their
    /// whole shingle set (Jaccard 1.0 ≥ any clone-similarity threshold).
    #[test]
    fn rename_equivalent_functions_share_all_shingles() {
        let original = r#"
fn compute(values: &[u32]) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > 10 {
            total += value * 2;
        }
    }
    total
}
"#;
        // Same body, every identifier renamed and the literals kept structural.
        let renamed = r#"
fn tally(items: &[u32]) -> u32 {
    let mut sum = 0;
    for item in items {
        if *item > 10 {
            sum += item * 2;
        }
    }
    sum
}
"#;
        let a: BTreeSet<u64> = shingles_of(original).into_iter().collect();
        let b: BTreeSet<u64> = shingles_of(renamed).into_iter().collect();
        assert!(!a.is_empty(), "a non-trivial body produces shingles");
        assert_eq!(
            a, b,
            "renaming identifiers must not change the shingle set (FR-EX-09)"
        );
    }

    /// FR-EX-09: an unrelated function shares few or no shingles with the pair.
    #[test]
    fn an_unrelated_function_does_not_share_the_clone_set() {
        let original = r#"
fn compute(values: &[u32]) -> u32 {
    let mut total = 0;
    for value in values {
        if *value > 10 {
            total += value * 2;
        }
    }
    total
}
"#;
        let unrelated = r#"
fn greet(name: &str) -> String {
    let mut out = String::new();
    out.push_str("hello ");
    out.push_str(name);
    out
}
"#;
        let a: BTreeSet<u64> = shingles_of(original).into_iter().collect();
        let c: BTreeSet<u64> = shingles_of(unrelated).into_iter().collect();
        let shared = a.intersection(&c).count();
        let union = a.union(&c).count();
        let jaccard = shared as f64 / union as f64;
        assert!(
            jaccard < 0.5,
            "an unrelated function must score well below a clone threshold (got {jaccard})"
        );
    }

    /// FR-EX-09: a body below the K_GRAM-token floor produces no shingles.
    #[test]
    fn a_trivial_body_is_below_the_token_floor() {
        assert!(
            shingles_of("fn f() {}").is_empty(),
            "an empty body has no k-gram"
        );
    }

    /// NFR-RA-06: the fingerprint is byte-stable across repeated computation and
    /// already sorted+deduplicated (one canonical representation).
    #[test]
    fn shingles_are_deterministic_and_canonical() {
        let src = r#"
fn f(xs: &[u32]) -> u32 {
    let mut acc = 0;
    for x in xs { acc += x; }
    acc
}
"#;
        let first = shingles_of(src);
        let second = shingles_of(src);
        assert_eq!(first, second, "deterministic across runs");
        let mut sorted = first.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(first, sorted, "returned set is sorted and deduplicated");
    }
}
