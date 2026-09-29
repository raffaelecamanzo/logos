//! Fixtures for the declared-contract relation's pure derivation: document
//! identity at and below the threshold, the equal-score collision, the copy
//! grouping and its naming, the member kinds, and the tie resolution scoped to
//! the holder's document.

use super::*;

/// The symbol of the `n`th operation in `path` — a file descriptor exactly as a
/// spec extractor emits one (`dir/`, then the backticked file name).
fn symbol(path: &str, n: usize, method: &str) -> LogosSymbol {
    let (dir, file) = path.rsplit_once('/').map_or(("", path), |(d, f)| (d, f));
    let dir = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    LogosSymbol::parse(&format!("logos . . . {dir}`{file}`/op{n}#{}#", method.to_lowercase()))
        .expect("fixture symbol parses")
}

/// One operation of `member`'s `path`, with the tier's verdict.
fn op(member: &str, path: &str, n: usize, (method, template): (&str, &str), providers: OperationProviders) -> SpecOperation {
    SpecOperation {
        member: member.into(),
        symbol: symbol(path, n, method),
        key: route_key_of(method, template),
        providers,
    }
}

fn route_key_of(method: &str, template: &str) -> Option<OperationKey> {
    crate::resolve::route_template::route_key(&format!("{method} {template}"))
}

/// Every operation of one document, all under the same verdict.
fn document(member: &str, path: &str, ops: &[(&str, &str)], providers: &OperationProviders) -> Vec<SpecOperation> {
    ops.iter().enumerate().map(|(n, o)| op(member, path, n, *o, providers.clone())).collect()
}

/// `n` distinct operations `GET /r{i}` from `from`.
fn ops(from: usize, n: usize) -> Vec<(&'static str, String)> {
    (from..from + n).map(|i| ("GET", format!("/r{i}"))).collect()
}

fn borrowed<'a>(ops: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    ops.iter().map(|(m, t)| (*m, t.as_str())).collect()
}

/// An own spec (served by its holder) and a vendored one (served by nobody).
fn own(member: &str, path: &str, o: &[(&'static str, String)]) -> Vec<SpecOperation> {
    document(member, path, &borrowed(o), &OperationProviders::Holder)
}
fn vendored(member: &str, path: &str, o: &[(&'static str, String)]) -> Vec<SpecOperation> {
    document(member, path, &borrowed(o), &OperationProviders::Elsewhere)
}

fn no_kinds() -> BTreeMap<String, MemberKind> {
    BTreeMap::new()
}
fn kinds(k: &[(&str, MemberKind)]) -> BTreeMap<String, MemberKind> {
    k.iter().map(|(m, k)| ((*m).to_string(), *k)).collect()
}
fn no_titles(_: &str, _: &str) -> Option<String> {
    None
}

fn endpoint(member: &str, path: &str, n: usize) -> BridgeEndpoint {
    BridgeEndpoint { member: member.into(), symbol: symbol(path, n, "GET") }
}

// ── document identity ───────────────────────────────────────────────────────

/// **The threshold.** A held document 9 of whose 10 operations are in `agg`'s
/// own spec is `agg`'s by identity, named with the score and the matched
/// document; at 8 of 10 it is not, and it falls to a named external.
#[test]
fn document_identity_needs_ninety_percent_of_the_held_document() {
    for (shared, expect_member) in [(9, true), (8, false)] {
        let mut held = ops(0, shared);
        held.extend(ops(100, 10 - shared)); // operations agg does not serve
        let mut facts = own("agg", "api/v1.yaml", &ops(0, 12));
        facts.extend(vendored("web", "spec/agg.yaml", &held));

        let r = derive(&facts, &no_kinds(), no_titles);
        assert_eq!(r.contracts.len(), 1, "one vendored document at {shared}/10");
        let target = &r.contracts[0].target;
        if expect_member {
            assert_eq!(
                target,
                &ContractTarget::Member {
                    member: "agg".into(),
                    document: "api/v1.yaml".into(),
                    shared: 9,
                    total: 10,
                },
                "9 of 10 is document identity"
            );
            assert_eq!((r.headline.to_member, r.headline.to_external), (1, 0));
        } else {
            assert!(
                matches!(target, ContractTarget::External { .. }),
                "8 of 10 is below identity: {target:?}"
            );
            assert_eq!((r.headline.to_member, r.headline.to_external), (0, 1));
        }
        assert_eq!(r.contracts[0].provenance, VENDORED_SPEC);
    }
}

/// Each member's best own spec is scored, and the best member wins. The weaker
/// spec sorts first, so taking the first qualifying spec instead of the best
/// one names the wrong document.
#[test]
fn identity_takes_the_best_member_and_each_members_best_spec() {
    let mut facts = own("agg", "a/a-small.yaml", &ops(0, 9));
    facts.extend(own("agg", "a/z-full.yaml", &ops(0, 10)));
    let mut partial = ops(0, 9);
    partial.extend(ops(50, 1));
    facts.extend(own("core", "c/v1.yaml", &partial));
    facts.extend(vendored("web", "w/agg.yaml", &ops(0, 10)));

    let r = derive(&facts, &no_kinds(), no_titles);
    assert_eq!(
        r.contracts[0].target,
        ContractTarget::Member { member: "agg".into(), document: "a/z-full.yaml".into(), shared: 10, total: 10 }
    );
}

/// **Two members at an equal best score resolve to neither.** The collision is
/// reported, and the document falls through to its named external.
#[test]
fn an_equal_best_score_resolves_to_neither_member() {
    let mut facts = own("agg", "a/v1.yaml", &ops(0, 10));
    facts.extend(own("core", "c/v1.yaml", &ops(0, 10)));
    facts.extend(vendored("web", "w/api.yaml", &ops(0, 10)));

    let r = derive(&facts, &no_kinds(), no_titles);
    assert!(
        matches!(r.contracts[0].target, ContractTarget::External { .. }),
        "an equal-score tie names neither member: {:?}",
        r.contracts[0].target
    );
    assert_eq!(
        r.collisions,
        vec![IdentityCollision {
            holder: "web".into(),
            document: "w/api.yaml".into(),
            members: vec!["agg".into(), "core".into()],
            shared: 10,
            total: 10,
        }]
    );
    assert_eq!(r.headline.identity_collisions, 1);
    assert_eq!(r.headline.to_member, 0);
}

/// A holder never identifies itself, and a document it serves at all is not
/// vendored.
#[test]
fn only_a_document_its_holder_serves_none_of_is_vendored() {
    let mut facts = own("agg", "a/v1.yaml", &ops(0, 10));
    // 1 of 10 served: partial, declares nothing.
    let mut partial = document("web", "w/partial.yaml", &borrowed(&ops(0, 1)), &OperationProviders::Holder);
    partial.extend(
        ops(1, 9)
            .iter()
            .enumerate()
            .map(|(n, (m, t))| op("web", "w/partial.yaml", n + 1, (m, t), OperationProviders::Elsewhere)),
    );
    facts.extend(partial);
    // A document with no keyed operation is unjudged.
    facts.push(op("web", "w/empty.yaml", 0, ("GET", "not-a-path"), OperationProviders::Elsewhere));

    let r = derive(&facts, &no_kinds(), no_titles);
    assert!(r.contracts.is_empty(), "nothing is vendored: {:?}", r.contracts);
    assert_eq!(
        r.headline.documents,
        DocumentAccounting { documents: 3, own: 1, partial: 1, unjudged: 1, ..Default::default() }
    );
    assert!(r.is_empty());
}

// ── named externals ─────────────────────────────────────────────────────────

/// Copies no member implements group by containment into ONE external — a
/// newer version holding the older's operations is the same external — named
/// by the copies' title, and declared by each holder.
#[test]
fn copies_group_into_one_external_named_by_their_title() {
    let mut facts = vendored("web", "w/pss-v1.yaml", &ops(0, 10));
    facts.extend(vendored("web", "w/pss-v2.yaml", &ops(0, 12)));
    facts.extend(vendored("facade", "f/pss.yaml", &ops(0, 10)));
    facts.extend(vendored("notify", "n/gateway.yaml", &ops(200, 3)));
    let titles = |member: &str, path: &str| match (member, path) {
        ("web", "w/pss-v1.yaml") => Some("PSS".to_string()),
        ("facade", _) => Some("PSS".to_string()),
        ("web", "w/pss-v2.yaml") => Some("PSS v2".to_string()),
        _ => None,
    };

    let r = derive(&facts, &no_kinds(), titles);
    let names: Vec<(&str, &str)> = r.externals.iter().map(|e| (e.id.0.as_str(), e.name.as_str())).collect();
    assert_eq!(
        names,
        vec![("facade:f/pss.yaml", "PSS"), ("notify:n/gateway.yaml", "gateway")],
        "one PSS external across three copies, named by the most common title; \
         the untitled one by its stem"
    );
    let pss = &r.externals[0];
    assert_eq!(pss.copies.len(), 3);
    assert_eq!(pss.declared_by, vec!["facade".to_string(), "web".to_string()]);
    // Two web copies of one external are ONE pair.
    assert_eq!(r.contracts.len(), 4);
    assert_eq!(r.headline.declared_contract_pairs, 3);
    assert_eq!((r.headline.to_member, r.headline.to_external), (0, 3));
    assert_eq!(
        r.externals_declared_by("web").map(|(c, e)| (c.document.as_str(), e.name.as_str())).collect::<Vec<_>>(),
        vec![("w/pss-v1.yaml", "PSS"), ("w/pss-v2.yaml", "PSS")],
        "the join S-459 reads: each copy the member holds, with its own operations"
    );
    assert_eq!(r.contracts_of("web").next().unwrap().operations.len(), 10);
}

/// springdoc's default title names nothing; a real title beats it; with none,
/// the first copy's file stem names the external.
#[test]
fn an_external_is_named_by_its_title_unless_it_is_the_springdoc_default() {
    assert_eq!(external_name(&[("a/v1.yaml", Some(SPRINGDOC_DEFAULT_TITLE))]), "v1");
    assert_eq!(
        external_name(&[("b/x.yaml", Some(SPRINGDOC_DEFAULT_TITLE)), ("a/y.json", Some("Gateway"))]),
        "Gateway"
    );
    assert_eq!(external_name(&[("b/x.yaml", None), ("a/y.json", Some(""))]), "y");
    assert_eq!(external_name(&[("b/x.yaml", None), ("a/y.json", None)]), "y");
}

/// The title is read textually from YAML and JSON, and a path that would leave
/// the member root is refused rather than followed.
#[test]
fn a_spec_title_is_read_from_the_members_own_file_only() {
    assert_eq!(spec_title("openapi: 3.0.1\ninfo:\n  title: PSS\n  version: 1\n").as_deref(), Some("PSS"));
    assert_eq!(spec_title("info:\n  title: \"Notification Gateway\"\n").as_deref(), Some("Notification Gateway"));
    assert_eq!(spec_title(r#"{"info": {"title": "Poste", "version": "1"}}"#).as_deref(), Some("Poste"));
    assert_eq!(spec_title("paths: {}\n"), None);
    assert_eq!(spec_title("info:\n  title: 'PSS'\n").as_deref(), Some("PSS"), "single-quoted");
    // `title` is the key under `info`, never a word before it (review: Swagger
    // 2 puts `description` first) — in YAML and in JSON.
    assert_eq!(
        spec_title("swagger: \"2.0\"\ninfo:\n  description: \"Every mailbox entitled to a quota\"\n  version: \"1.0.0\"\n  title: \"Mail API\"\n").as_deref(),
        Some("Mail API")
    );
    assert_eq!(
        spec_title(r#"{"swagger":"2.0","info":{"description":"Subtitles service","version":"1","title":"Subs API"}}"#).as_deref(),
        Some("Subs API")
    );
    assert_eq!(spec_title("info:\n  contact:\n    title: Nested\n"), None, "a nested `title` is not info's");
    // Forms the YAML subset does not bind name nothing, never a partial value.
    assert_eq!(spec_title("openapi: 3.0.0\ninfo: {title: PSS, version: '1'}\npaths: {}\n"), None);
    assert_eq!(spec_title("openapi: 3.0.0\ninfo:\n  title: >-\n    Notification Gateway\n  version: '1'\n"), None);

    let tmp = tempfile::tempdir().expect("tempdir");
    let member = tmp.path().join("m");
    std::fs::create_dir_all(member.join("spec")).unwrap();
    std::fs::write(member.join("spec/api.yaml"), "info:\n  title: Inside\n").unwrap();
    std::fs::write(tmp.path().join("outside.yaml"), "info:\n  title: Outside\n").unwrap();
    assert_eq!(read_title(&member, "spec/api.yaml").as_deref(), Some("Inside"));
    assert_eq!(read_title(&member, "../outside.yaml"), None, "`..` is refused");
    assert_eq!(read_title(&member, "spec/missing.yaml"), None);
}

// ── member kinds ────────────────────────────────────────────────────────────

/// **A mock is a stand-in provider, never a consumer.** Its copy — served by
/// its own routes or not — joins the external it mocks and declares nothing,
/// and a document matching the mock's served spec is never the mock's by
/// identity.
#[test]
fn a_mock_holder_stands_in_and_declares_nothing() {
    let mut facts = own("pss-mock", "m/pss.yaml", &ops(0, 10));
    facts.extend(vendored("facade", "f/pss.yaml", &ops(0, 10)));
    facts.extend(vendored("gw-mock", "g/gw.yaml", &ops(200, 4)));

    let undeclared = derive(&facts, &no_kinds(), no_titles);
    assert_eq!(
        undeclared.contracts.iter().find(|c| c.holder == "facade").unwrap().target,
        ContractTarget::Member { member: "pss-mock".into(), document: "m/pss.yaml".into(), shared: 10, total: 10 },
        "undeclared, the mock's served spec is an own spec a copy can identify"
    );

    let declared = kinds(&[("pss-mock", MemberKind::Mock), ("gw-mock", MemberKind::Mock)]);
    let r = derive(&facts, &declared, no_titles);
    assert!(r.contracts.iter().all(|c| !c.holder.ends_with("-mock")), "a mock declares nothing");
    let facade = r.contracts.iter().find(|c| c.holder == "facade").unwrap();
    let ContractTarget::External { external, .. } = &facade.target else {
        panic!("a mock is never the member a document identifies: {:?}", facade.target);
    };
    let pss = r.external(external).unwrap();
    assert_eq!(pss.stand_ins, vec!["pss-mock".to_string()]);
    assert_eq!(pss.declared_by, vec!["facade".to_string()]);
    let gw = r.externals.iter().find(|e| e.stand_ins == vec!["gw-mock".to_string()]).unwrap();
    assert!(gw.declared_by.is_empty(), "stood in for by a mock, declared by nobody");
    assert_eq!(r.headline.documents.mock, 2);
    assert_eq!(r.headline.declared_contract_pairs, 1);
}

/// **A documentation holder stays out**: its copies declare nothing, join no
/// external, and its own-looking specs identify nothing — only the accounting
/// counts them.
#[test]
fn a_documentation_holder_stays_out() {
    let mut facts = vendored("docs", "d/pss.yaml", &ops(0, 10));
    facts.extend(own("docs", "d/served.yaml", &ops(300, 5)));
    facts.extend(vendored("facade", "f/pss.yaml", &ops(0, 10)));
    facts.extend(vendored("web", "w/served.yaml", &ops(300, 5)));

    let r = derive(&facts, &kinds(&[("docs", MemberKind::Documentation)]), no_titles);
    assert!(r.contracts.iter().all(|c| c.holder != "docs"));
    assert!(
        r.externals.iter().flat_map(|e| &e.copies).all(|c| c.member != "docs"),
        "no documentation copy joins an external: {:?}",
        r.externals
    );
    assert!(r
        .contracts
        .iter()
        .all(|c| !matches!(c.target, ContractTarget::Member { .. })), "a documentation spec identifies nothing");
    assert_eq!(r.headline.documents.documentation, 2);
    assert_eq!(r.headline.declared_contract_pairs, 2);
}

/// A `platform` member is an ordinary one: it declares, and it is identified.
#[test]
fn a_platform_member_is_an_ordinary_holder() {
    let mut facts = own("common", "c/v1.yaml", &ops(0, 10));
    facts.extend(vendored("starter", "s/common.yaml", &ops(0, 10)));
    let declared = kinds(&[("common", MemberKind::Platform), ("starter", MemberKind::Platform)]);
    let r = derive(&facts, &declared, no_titles);
    assert_eq!(r.pairs().into_iter().collect::<Vec<_>>(), vec![("starter", Counterparty::Member("common"))]);
}

// ── tie resolution ──────────────────────────────────────────────────────────

/// **The identity resolves its document's ties — for the holder's document
/// only.** `web`'s copy of `agg`'s spec has three tied operations: one tied
/// between `agg` and `core` resolves to `agg`; one tied between `core` and
/// `other` does not (the identity says nothing about them); one where `agg`
/// is two candidates does not (exactly one, or nothing). `web`'s second
/// document, which identifies nothing, has a tie involving `agg` too — it is
/// not resolved.
#[test]
fn ties_resolve_for_the_holders_document_only() {
    let doc = "w/agg.yaml";
    let mut facts = own("agg", "a/v1.yaml", &ops(0, 10));
    let tied = |candidates: &[(&str, usize)]| {
        OperationProviders::Tied(candidates.iter().map(|(m, n)| endpoint(m, "a/v1.yaml", *n)).collect())
    };
    let held = ops(0, 10);
    for (n, (m, t)) in held.iter().enumerate() {
        let providers = match n {
            0 => tied(&[("agg", 0), ("core", 0)]),
            1 => tied(&[("core", 1), ("other", 1)]),
            2 => tied(&[("agg", 2), ("agg", 7), ("core", 2)]),
            _ => OperationProviders::Elsewhere,
        };
        facts.push(op("web", doc, n, (m, t), providers));
    }
    // A second web document that identifies nothing, with a tie involving agg.
    facts.push(op("web", "w/other.yaml", 0, ("GET", "/r900"), tied(&[("agg", 0), ("core", 0)])));

    let r = derive(&facts, &no_kinds(), no_titles);
    assert_eq!(
        r.resolved_ties,
        vec![ResolvedTie {
            holder: "web".into(),
            document: doc.into(),
            operation: symbol(doc, 0, "GET"),
            provider: endpoint("agg", "a/v1.yaml", 0),
        }]
    );
    assert_eq!(r.headline.resolved_ties, 1);
    assert!(r.resolved_tie("web", &symbol(doc, 0, "GET")).is_some());
    assert!(r.resolved_tie("web", &symbol("w/other.yaml", 0, "GET")).is_none());
}

/// A document's operations are the file its symbols name; a `local` symbol
/// names none and is in no document.
#[test]
fn a_document_path_is_the_symbols_file_descriptor() {
    let p = |s: &str| document_path(&LogosSymbol::parse(s).unwrap());
    assert_eq!(p("logos . . . api/src/main/resources/openapi/`v1.yaml`/op0#get#").as_deref(), Some("api/src/main/resources/openapi/v1.yaml"));
    assert_eq!(p("logos . . . `PSS_v1.0.3.yaml`/op0#get#").as_deref(), Some("PSS_v1.0.3.yaml"));
    assert_eq!(p("local op0"), None);
}

/// The headline is stated over its denominator, which accounts for every
/// document, and says what it is not.
#[test]
fn the_headline_is_stated_over_every_spec_document_read() {
    let mut facts = own("agg", "a/v1.yaml", &ops(0, 10));
    facts.extend(vendored("web", "w/agg.yaml", &ops(0, 10)));
    facts.extend(vendored("web", "w/pss.yaml", &ops(100, 4)));
    facts.extend(vendored("docs", "d/pss.yaml", &ops(100, 4)));
    let r = derive(&facts, &kinds(&[("docs", MemberKind::Documentation)]), no_titles);
    let d = &r.headline.documents;
    assert_eq!(d.documents, d.own + d.vendored + d.partial + d.unjudged + d.mock + d.documentation);
    assert_eq!(
        r.headline.summary,
        "2 declared contract pairs (1 by document identity, 1 to named externals) from 2 vendored \
         of 4 spec documents; 1 named externals; 0 contract-surface ties resolved by document \
         identity; declared by vendored specs, never observed calls"
    );
}
