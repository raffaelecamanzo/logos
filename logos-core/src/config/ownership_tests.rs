//! Routing of `config.toml` tables and `secrets.toml` keys ([FR-CF-09]).
//!
//! Every test here routes against explicit [`Claims`], never the process
//! registry, so they are independent of each other and of the tests that do
//! register (`writeback_ownership_tests.rs`).
//!
//! Since [S-643] the core no longer owns `[chat]` or `chat.api_key`, and since
//! [S-644] it no longer owns `[wiki]`, so the real [`Config`] and [`Secrets`]
//! exercise moved tables. Some cases parse as a minimal core type that declares
//! neither (`CoreAfterMove`), to isolate the routing from the core's own fields.
//!
//! [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md
//! [S-643]: ../../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
//! [S-644]: ../../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension

use std::sync::Arc;

use super::*;
use crate::config::secrets::parse_secrets;
use crate::config::{ChatConfig, WikiConfig};

const PATH: &str = ".logos/config.toml";

/// The `config.toml` core once `[chat]` and `[wiki]` have left it.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct CoreAfterMove {
    #[serde(default)]
    max_file_size: Option<u64>,
    #[serde(default)]
    languages: Vec<String>,
}

/// The `secrets.toml` core once `[chat]` has left it: no secret at all.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SecretsAfterMove {}

/// An extension table schema: `greeting`, a string, and nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ProbeTable {
    greeting: String,
}

fn probe_claims() -> Claims {
    Claims::new(
        &[TableClaim::new("probe", "probe", |value: &TomlValue| {
            value.clone().try_into::<ProbeTable>().map(drop).map_err(|e| e.message().to_string())
        })],
        &[SecretClaim {
            owner: "probe",
            key: "probe.token",
        }],
    )
    .expect("acceptable claims")
}

fn route<T: DeserializeOwned>(text: &str, claims: &Claims) -> Result<Routed<T, ()>, ConfigError> {
    route_config::<T>(text, Path::new(PATH), claims)
}

/// Today's error for `text`: the core's own parse, unrouted.
fn todays_error(text: &str) -> String {
    let source = toml::from_str::<Config>(text).expect_err("the core rejects it");
    ConfigError::Parse {
        path: PATH.into(),
        source,
    }
    .to_string()
}

// ── config.toml: an extension's table ──────────────────────────────────────

#[test]
fn an_extension_table_validates_through_its_extension() {
    let claims = probe_claims();
    let ok = route::<Config>(
        "max_file_size = 42\n\n[probe]\ngreeting = \"hi\"\n\n[chat]\nmodel = \"m\"\n",
        &claims,
    )
    .expect("the extension accepts its table");
    assert_eq!(ok.value.max_file_size, 42, "the core's keys still parse");
    assert_eq!(ok.value.chat, ChatConfig::default(), "the core's own parse no longer reads [chat]");
    assert!(ok.ignored.is_empty(), "[chat] is still read by the core (S-643), so never ignored");
    assert_eq!(ok.reserved["chat"]["model"].as_str(), Some("m"), "and its value is carried out");

    let err = route::<Config>("[probe]\ngreting = \"typo\"\n", &claims)
        .expect_err("the extension rejects a key its schema lacks")
        .to_string();
    assert!(
        err.starts_with("invalid value for probe: unknown field `greting`"),
        "the extension's own message, attributed to the table: {err}"
    );
    assert!(err.contains("the `probe` extension"), "names the owner: {err}");
    assert!(err.ends_with(&format!("in {PATH})")), "and the file, as a parse error does: {err}");
}

#[test]
fn without_a_claim_an_extension_table_is_an_unknown_key_with_todays_error() {
    let text = "[probe]\ngreeting = \"hi\"\n";
    let err = route::<Config>(text, &Claims::default()).expect_err("nobody owns [probe]");
    assert_eq!(err.to_string(), todays_error(text));
}

#[test]
fn a_core_error_is_reported_before_an_extension_error() {
    let text = "[probe]\ngreting = 1\n\n[documentation]\nenabld = true\n";
    let err = route::<Config>(text, &probe_claims()).expect_err("both are wrong");
    assert!(err.to_string().contains("unknown field `enabld`"), "{err}");
}

// ── config.toml: reserved tables nobody claims ─────────────────────────────

#[test]
fn without_its_owner_a_reserved_table_is_ignored_and_named() {
    let text = "max_file_size = 7\n\n[chat]\nmodel = \"m\"\nanything = { goes = 1 }\n\n\
                [wiki]\nmodel = \"w\"\n";
    let routed = route::<CoreAfterMove>(text, &Claims::default()).expect("reserved tables parse");
    assert_eq!(
        routed.value,
        CoreAfterMove {
            max_file_size: Some(7),
            languages: Vec::new()
        }
    );
    // `[chat]` is routed away but read by the core (S-643), so only `[wiki]` is
    // reported as ignored.
    assert_eq!(routed.ignored, ["wiki"]);
    assert!(routed.reserved.contains_key("chat") && routed.reserved.contains_key("wiki"));
}

/// `[wiki]` left the core in S-644: claimed by the `wiki` extension, its owner
/// validates it and the core reads what the owner accepted; unclaimed (a build
/// without the extension), it is ignored and reported, never parsed.
#[test]
fn the_wiki_table_is_read_from_its_owner_and_ignored_without_one() {
    let text = "[chat]\nmodel = \"m\"\n\n[wiki]\nmodel = \"w\"\n";
    let unclaimed = route::<Config>(text, &Claims::default()).expect("parses");
    assert_eq!(unclaimed.ignored, ["wiki"], "still reads [chat] (S-643), ignores [wiki]");
    assert_eq!(unclaimed.value.wiki, WikiConfig::default(), "nothing of it reaches the core's parse");

    let claimed = route::<Config>(text, &wiki_claim()).expect("its owner accepts it");
    assert!(claimed.ignored.is_empty());
    assert_eq!(WikiConfig::from_claimed(claimed.reserved.get("wiki")).model.as_deref(), Some("w"));
}

/// The core reads `[chat]` whoever owns it (S-643), and never reports it ignored:
/// claimed, its owner validates it — the error naming the key and the file — and
/// the core reads what the owner accepted; unclaimed, the core parses it exactly
/// as it did while it owned it, with today's error and position.
#[test]
fn the_core_reads_the_chat_table_whoever_owns_it() {
    let chat_claim = Claims::new(
        &[TableClaim::new("chat", "chat", ChatConfig::validate_table)],
        &[SecretClaim { owner: "chat", key: "chat.api_key" }],
    )
    .expect("acceptable");
    let text = "[chat]\nmodel = \"m\"\nmax_tool_calls = 30\n";
    let read = |chat: ChatConfig| (chat.model, chat.max_tool_calls);

    let claimed = route::<Config>(text, &chat_claim).expect("its owner accepts it");
    assert!(claimed.ignored.is_empty());
    assert_eq!(read(ChatConfig::from_claimed(claimed.reserved.get("chat"))), (Some("m".to_string()), 30));
    let unclaimed = route::<Config>(text, &Claims::default()).expect("routed away");
    assert!(unclaimed.ignored.is_empty(), "read by the core, so never ignored");
    assert_eq!(read(ChatConfig::from_document(text, Path::new(PATH)).expect("valid")), (Some("m".to_string()), 30));

    let err = route::<Config>("[chat]\nmax_tool_calls = 0\n", &chat_claim).expect_err("validated");
    assert!(
        err.to_string().starts_with("invalid value for chat: chat.max_tool_calls: must be at least 1"),
        "{err}"
    );
    assert!(err.to_string().ends_with(&format!("in {PATH})")), "{err}");

    let err = ChatConfig::from_document("[chat]\nmodel = \"m\"\nmodle = 1\n", Path::new(PATH))
        .expect_err("the core's own parse rejects a typo");
    assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
    assert!(err.to_string().contains("line 3") && err.to_string().contains("unknown field `modle`"), "{err}");
    let err = ChatConfig::from_document("[chat]\nmax_tool_calls = 0\n", Path::new(PATH)).expect_err("bounds");
    assert!(matches!(err, ConfigError::InvalidValue { ref key, .. } if key == "chat.max_tool_calls"), "{err:?}");
}

#[test]
fn an_unknown_non_reserved_table_still_fails_with_todays_error() {
    // Routing something away (the [probe] table, a reserved table) must not
    // move or reword the core's error for what it still rejects.
    for text in [
        "[bogus]\nx = 1\n",
        "[bogus]\nx = 1\n\n[probe]\ngreeting = \"hi\"\n",
        "max_file_size = 1\nlangauges = []\n\n[probe]\ngreeting = \"hi\"\n",
        "[probe]\ngreeting = \"hi\"\n\n[semantics]\nignored_dirs = 3\n",
        "max_file_size = \"big\"\n\n[probe]\ngreeting = \"hi\"\n",
        "not toml at all [\n",
    ] {
        let err = route::<Config>(text, &probe_claims()).expect_err(text);
        let expected = todays_error(&text.replace("[probe]\ngreeting = \"hi\"\n", "\n\n"));
        assert_eq!(err.to_string(), expected, "for {text:?}");
    }
    // And a reserved table the core no longer owns does not hide an unknown
    // sibling either.
    let text = "[chat]\nmodel = \"m\"\n\n[bogus]\nx = 1\n";
    let err = route::<CoreAfterMove>(text, &Claims::default()).expect_err("bogus is unknown");
    let expected = toml::from_str::<CoreAfterMove>("\n\n\n[bogus]\nx = 1\n").expect_err("x");
    assert_eq!(
        err.to_string(),
        ConfigError::Parse {
            path: PATH.into(),
            source: expected
        }
        .to_string()
    );
}

/// The `wiki` extension's claim: its table, validated as the core validated it.
fn wiki_claim() -> Claims {
    Claims::new(&[TableClaim::new("wiki", "wiki", WikiConfig::validate_table)], &[])
        .expect("acceptable")
}

/// In the default build — the `wiki` extension claiming `[wiki]` (S-644) — the
/// table reads exactly as it did while the core owned it, and what the core
/// rejected its owner rejects, the error naming the key and the file.
#[test]
fn the_wiki_resolves_exactly_as_today_in_the_default_build() {
    /// The table as the core parsed it before the move.
    #[derive(Deserialize)]
    struct WikiOnly {
        #[serde(default)]
        wiki: WikiConfig,
    }
    for (text, rejected) in [
        ("", None),
        ("[wiki]\n", None),
        ("[wiki]\nmodel = \"w\"\nrevision_stale_threshold = 3\n", None),
        ("[wiki]\nmodel = 3\n", Some("invalid type")),
        ("[wiki]\nbogus = 1\n", Some("unknown field")),
        ("[wiki]\nmodel = \" \"\n", Some("wiki.model: must not be empty")),
        ("[wiki]\nrevision_stale_threshold = 0\n", Some("wiki.revision_stale_threshold: must be at least 1")),
    ] {
        let routed = route::<Config>(text, &wiki_claim());
        match rejected {
            None => {
                let routed = routed.expect(text);
                let today = toml::from_str::<WikiOnly>(text).expect(text).wiki;
                assert_eq!(WikiConfig::from_claimed(routed.reserved.get("wiki")), today, "{text:?}");
                assert!(routed.ignored.is_empty());
            }
            Some(expected) => {
                let err = routed.expect_err(text).to_string();
                assert!(err.starts_with("invalid value for wiki: "), "{err}");
                assert!(err.contains(expected), "{text:?}: {err}");
                assert!(err.ends_with(&format!("(the table belongs to the `wiki` extension; in {PATH})")), "{err}");
            }
        }
    }
}

// ── secrets.toml ───────────────────────────────────────────────────────────

fn route_s<T: DeserializeOwned>(
    text: &str,
    claims: &Claims,
) -> Result<Routed<T, BTreeMap<String, String>>, ConfigError> {
    route_secrets::<T>(text, Path::new(".logos/secrets.toml"), claims, Clone::clone)
}

#[test]
fn an_extension_secret_routes_to_its_extension_and_the_core_keeps_its_own() {
    let routed = route_s::<Secrets>(
        "[chat]\napi_key = \"sk-core-1234\"\n\n[probe]\ntoken = \"tok-ext-9876\"\n",
        &probe_claims(),
    )
    .expect("parses");
    assert_eq!(
        routed.extension,
        BTreeMap::from([("probe.token".to_string(), "tok-ext-9876".to_string())])
    );
    // `chat.api_key` may be the `chat` extension's since S-643: unclaimed here, its
    // table is routed away from the core's own parse — and read, never ignored.
    assert!(routed.ignored.is_empty());
    assert_eq!(routed.reserved["chat"]["api_key"].as_str(), Some("sk-core-1234"));
}

#[test]
fn an_extension_secret_table_admits_only_claimed_string_keys_and_never_echoes_a_value() {
    let claims = probe_claims();
    // A pasted secret in the key position: nobody owns it, and its name is not
    // echoed, as the core's redacted parse would not echo it either.
    let err = route_s::<Secrets>("[probe]\ntoken = \"t\"\nsk-live-0123456789 = \"x\"\n", &claims)
        .expect_err("probe.sk-live-… is nobody's");
    assert_eq!(
        err.to_string(),
        "invalid value for probe: holds a key no active extension owns (the key is not echoed; \
         in .logos/secrets.toml)"
    );
    let err = route_s::<Secrets>("[probe]\ntoken = 1234\n", &claims).expect_err("not a string");
    assert!(!err.to_string().contains("1234"), "{err}");
    let err = route_s::<Secrets>("probe = \"s3cr3t\"\n", &claims).expect_err("not a table");
    assert!(!err.to_string().contains("s3cr3t"), "{err}");
}

#[test]
fn an_extension_secret_is_trimmed_and_a_blank_one_is_unset_like_the_chat_key() {
    use crate::config::MaskedSecret;
    let secrets_of = |text: &str| Secrets {
        extension: route_s::<Secrets>(text, &probe_claims()).expect("parses").extension,
        ..Secrets::default()
    };
    let padded = secrets_of("[probe]\ntoken = \"  tok-1234  \"\n");
    assert_eq!(padded.extension_secret("probe.token"), Some("tok-1234"));
    assert_eq!(
        padded.extension_secrets_masked()["probe.token"],
        MaskedSecret::from_key(Some("tok-1234"))
    );
    let blank = secrets_of("[probe]\ntoken = \"   \"\n");
    assert_eq!(blank.extension_secret("probe.token"), None, "a blank secret is unset");
    assert!(!blank.extension_secrets_masked()["probe.token"].present);
}

#[test]
fn an_unknown_secret_table_still_fails_with_todays_redacted_error() {
    for text in ["[bogus]\nkey = \"s3cr3t\"\n", "[bogus]\nkey = \"s3cr3t\"\n\n[chat]\napi_key = \"k\"\n"] {
        let today = parse_secrets(text, Path::new("s.toml")).expect_err(text).to_string();
        let routed = route_secrets::<Secrets>(text, Path::new("s.toml"), &probe_claims(), |e| {
            super::super::secrets::redact_parse_error(e, text)
        })
        .expect_err(text)
        .to_string();
        assert_eq!(routed, today, "{text:?}");
        assert!(!routed.contains("s3cr3t"));
    }
}

/// The one reserved secret, `chat.api_key`, is read by the core (S-643): without
/// its owner it is routed away from the core's own parse but never ignored — the
/// core parses it itself (`parse_secrets`).
#[test]
fn without_its_owner_the_reserved_secret_is_routed_away_not_ignored() {
    let routed = route_s::<SecretsAfterMove>("[chat]\napi_key = \"sk-1234\"\n", &Claims::default())
        .expect("the reserved secret parses");
    assert!(routed.ignored.is_empty(), "read by the core, so never ignored");
    assert!(routed.extension.is_empty(), "an unclaimed secret is no extension's");
    let err = route_s::<SecretsAfterMove>("[other]\nk = \"v\"\n", &Claims::default())
        .expect_err("only the reserved table is tolerated");
    assert!(err.to_string().contains("unknown field `other`"), "{err}");
}

// ── registration ───────────────────────────────────────────────────────────

fn accept(_: &TomlValue) -> Result<(), String> {
    Ok(())
}

#[test]
fn a_claim_on_a_core_table_or_a_malformed_secret_key_is_refused() {
    for (tables, secrets, expected) in [
        (vec![TableClaim::new("x", "semantics", accept)], vec![], "claims the config table `[semantics]`, which the core owns"),
        (vec![TableClaim::new("x", "languages", accept)], vec![], "`[languages]`, which the core owns"),
        (vec![], vec![SecretClaim { owner: "x", key: "token" }], "which is not `table.key`"),
        (vec![], vec![SecretClaim { owner: "x", key: "a.b.c" }], "which is not `table.key`"),
        (vec![], vec![SecretClaim { owner: "x", key: ".b" }], "which is not `table.key`"),
    ] {
        let err = Claims::new(&tables, &secrets).expect_err(expected).to_string();
        assert!(err.contains(expected), "{err}");
    }
}

#[test]
fn two_owners_of_one_table_or_key_are_refused_and_nothing_is_admitted() {
    let mut claims = probe_claims();
    let err = claims
        .admit(
            &[
                TableClaim::new("other", "fresh", accept),
                TableClaim::new("other", "probe", accept),
            ],
            &[],
        )
        .expect_err("[probe] is taken")
        .to_string();
    assert_eq!(err, "the config table `[probe]` is claimed by both `probe` and `other`");
    assert!(claims.table("fresh").is_none(), "all or nothing");
    let err = claims
        .admit(&[], &[SecretClaim { owner: "other", key: "probe.token" }])
        .expect_err("probe.token is taken")
        .to_string();
    assert_eq!(err, "the secret key `probe.token` is claimed by both `probe` and `other`");
    // Re-registering what an owner already holds is a no-op.
    claims
        .admit(
            &[TableClaim::new("probe", "probe", accept)],
            &[SecretClaim { owner: "probe", key: "probe.token" }],
        )
        .expect("idempotent");
    assert_eq!((claims.tables.len(), claims.secrets.len()), (1, 1));
}

#[test]
fn two_owners_in_one_registration_are_refused() {
    // One host with two extensions registers in one call: the clash is inside
    // the batch, with nothing registered before it.
    let err = Claims::new(&[TableClaim::new("a", "t", accept), TableClaim::new("b", "t", accept)], &[])
        .expect_err("two owners of [t]")
        .to_string();
    assert_eq!(err, "the config table `[t]` is claimed by both `a` and `b`");
    let err = Claims::new(
        &[],
        &[SecretClaim { owner: "a", key: "t.k" }, SecretClaim { owner: "b", key: "t.k" }],
    )
    .expect_err("two owners of t.k")
    .to_string();
    assert_eq!(err, "the secret key `t.k` is claimed by both `a` and `b`");
}

#[test]
fn a_refused_secret_claim_admits_none_of_the_tables_or_secrets_beside_it() {
    let mut claims = probe_claims();
    claims
        .admit(&[TableClaim::new("other", "fresh", accept)], &[SecretClaim { owner: "other", key: "x" }])
        .expect_err("`x` is not `table.key`");
    assert!(claims.table("fresh").is_none(), "the table beside it is not admitted");
    claims
        .admit(
            &[],
            &[
                SecretClaim { owner: "other", key: "new.k" },
                SecretClaim { owner: "other", key: "bad" },
            ],
        )
        .expect_err("`bad` is not `table.key`");
    assert_eq!(claims.secrets.len(), 1, "nor the good secret before the bad one");
}

#[test]
fn the_core_keys_are_the_core_types_own_fields() {
    let config = fields_of::<Config>();
    for key in ["languages", "include", "max_file_size", "semantics"] {
        assert!(config.contains(&key), "{key} in {config:?}");
    }
    // `[chat]` and its secret left the core in S-643, `[wiki]` in S-644: skipped
    // by its parse, so they are not its fields, and their extensions may claim them.
    assert!(!config.contains(&"chat") && !config.contains(&"wiki"), "{config:?}");
    assert!(fields_of::<Secrets>().is_empty(), "the core owns no secret");
    assert!(fields_of::<toml::Table>().is_empty(), "not a struct");
}

// ── the notice ─────────────────────────────────────────────────────────────

#[test]
fn ignored_tables_are_reported_once_as_one_notice() {
    let reported = Mutex::new(BTreeSet::new());
    let config = Path::new(".logos/config.toml");
    let secrets = Path::new(".logos/secrets.toml");
    let tables = ["chat".to_string(), "wiki".to_string()];
    assert_eq!(
        first_notice(&reported, config, &tables).as_deref(),
        Some(
            "notice: .logos/config.toml declares [chat], [wiki], which no active extension \
             claims: ignored, and kept in the file"
        )
    );
    assert_eq!(first_notice(&reported, config, &tables), None, "once per process");
    assert_eq!(first_notice(&reported, secrets, &tables[..1]), None, "the same missing extension");
    assert_eq!(first_notice(&reported, config, &[]), None, "nothing ignored, nothing said");
}

#[test]
fn the_notice_is_one_warning_on_the_log_however_often_the_files_load() {
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let reported = Mutex::new(BTreeSet::new());
    let tables = ["chat".to_string(), "wiki".to_string()];
    tracing::subscriber::with_default(subscriber, || {
        // Sibling tests reach the callsite from threads with no subscriber.
        tracing::callsite::rebuild_interest_cache();
        report_ignored_in(&reported, Path::new(".logos/config.toml"), &tables);
        report_ignored_in(&reported, Path::new(".logos/config.toml"), &tables);
        report_ignored_in(&reported, Path::new(".logos/secrets.toml"), &tables[..1]);
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let notices: Vec<&str> = log.lines().filter(|l| l.contains("notice:")).collect();
    assert_eq!(notices.len(), 1, "one notice: {log}");
    assert!(notices[0].contains("WARN"), "a warning, so the default `warn` filter shows it: {log}");
    assert!(
        notices[0].contains("notice: .logos/config.toml declares [chat], [wiki], which no active extension claims"),
        "{log}"
    );
}

