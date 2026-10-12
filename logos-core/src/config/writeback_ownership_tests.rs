//! The read-model and the editors' writes over extension-owned and reserved
//! tables ([FR-CF-09]), through the public load and write paths.
//!
//! These register a probe extension's claims with the process — idempotently,
//! under names no other test uses (`ext_probe`, `ext_probe.token`) — exactly as
//! the composition root registers the host's, so `read_documents`,
//! `write_config` and `write_secret` route by them unmodified.
//!
//! [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md

use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::*;
use crate::config::{register_extension_config, SecretClaim, TableClaim, TomlValue};

/// The probe's table schema: `greeting`, a string, and nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ProbeTable {
    greeting: String,
}

fn register_probe() {
    register_extension_config(
        &[TableClaim::new("ext-probe", "ext_probe", |value: &TomlValue| {
            value.clone().try_into::<ProbeTable>().map(drop).map_err(|e| e.message().to_string())
        })],
        &[SecretClaim {
            owner: "ext-probe",
            key: "ext_probe.token",
        }],
    )
    .expect("the probe's claims register");
}

fn seed(root: &Path, name: &str, contents: &str) {
    let dir = root.join(".logos");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), contents).unwrap();
}

#[test]
fn an_extension_secret_is_masked_in_the_read_model_like_the_chat_key() {
    register_probe();
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "config.toml", "[ext_probe]\ngreeting = \"hi\"\n");
    seed(
        dir.path(),
        "secrets.toml",
        "[chat]\napi_key = \"sk-chat-key-1234\"\n\n[ext_probe]\ntoken = \"tok-probe-secret-9876\"\n",
    );
    let docs = read_documents(dir.path(), None).expect("the extension owns both");
    assert_eq!(docs.chat_key, MaskedSecret::from_key(Some("sk-chat-key-1234")));
    assert_eq!(
        docs.extension_secrets,
        BTreeMap::from([(
            "ext_probe.token".to_string(),
            MaskedSecret::from_key(Some("tok-probe-secret-9876"))
        )])
    );
    let json = serde_json::to_value(&docs).unwrap();
    assert_eq!(
        json["extension_secrets"]["ext_probe.token"],
        serde_json::json!({ "present": true, "last4": "9876" }),
        "masked exactly as `chat_key` is: {json}"
    );
    let text = json.to_string();
    assert!(!text.contains("tok-probe-secret"), "the raw secret never serialises: {text}");
    assert!(!format!("{docs:?}").contains("tok-probe-secret"), "nor renders in Debug");
}

#[test]
fn without_an_extension_secret_the_read_model_has_no_extension_field() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "secrets.toml", "[chat]\napi_key = \"sk-chat-key-1234\"\n");
    let json = serde_json::to_value(read_documents(dir.path(), None).unwrap()).unwrap();
    assert!(json.get("extension_secrets").is_none(), "unchanged read-model JSON: {json}");
}

#[test]
fn the_config_editor_keeps_an_extension_table_and_validates_it_through_the_extension() {
    register_probe();
    let dir = tempfile::tempdir().unwrap();
    let candidate = "[chat]\nmodel = \"m\"\n\n[ext_probe]\ngreeting = \"hi\"\n\n[wiki]\nmodel = \"w\"\n";
    write_config(dir.path(), candidate).expect("a valid extension table is written");
    let path = dir.path().join(CONFIG_RELPATH);
    assert_eq!(fs::read_to_string(&path).unwrap(), candidate, "kept verbatim");

    let err = write_config(dir.path(), "[ext_probe]\ngreeting = 3\n")
        .expect_err("the extension rejects it");
    assert!(err.to_string().starts_with("invalid value for ext_probe: invalid type"), "{err}");
    assert_eq!(fs::read_to_string(&path).unwrap(), candidate, "byte-identical on rejection");
}

#[test]
fn the_secret_editor_keeps_an_extension_secret_in_the_file() {
    register_probe();
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path(), "secrets.toml", "[ext_probe]\ntoken = \"tok-probe-5555\"\n");
    write_secret(dir.path(), "sk-new-chat-7777").expect("written");
    let secrets = load_secrets_from_root(dir.path()).unwrap();
    assert_eq!(secrets.chat_api_key(), Some("sk-new-chat-7777"));
    assert_eq!(secrets.extension_secret("ext_probe.token"), Some("tok-probe-5555"), "kept");
    let debug = format!("{secrets:?}");
    assert!(!debug.contains("tok-probe") && debug.contains("5555"), "redacted Debug: {debug}");
    write_secret(dir.path(), " ").expect("cleared");
    let secrets = load_secrets_from_root(dir.path()).unwrap();
    assert_eq!(secrets.chat_api_key(), None);
    assert_eq!(secrets.extension_secret("ext_probe.token"), Some("tok-probe-5555"), "still kept");
}

#[test]
fn the_secret_editor_writes_todays_bytes_when_the_store_holds_only_the_chat_key() {
    let dir = tempfile::tempdir().unwrap();
    write_secret(dir.path(), "sk-first-1111").unwrap();
    let path = dir.path().join(SECRETS_RELPATH);
    assert_eq!(fs::read_to_string(&path).unwrap(), "[chat]\napi_key = \"sk-first-1111\"\n");
    write_secret(dir.path(), "").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "[chat]\n");
}

#[test]
fn the_secret_editor_refuses_a_store_that_is_not_toml_with_the_redacted_load_error() {
    // The store is validated by the load path before it is edited, so its
    // fault is the redacted parse error, never the raw toml message.
    let dir = tempfile::tempdir().unwrap();
    let original = "api_key = sk-unquoted-9999\n[chat\n";
    seed(dir.path(), "secrets.toml", original);
    let err = write_secret(dir.path(), "sk-new-1111").expect_err("not TOML");
    assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
    assert!(!format!("{err} {err:?}").contains("sk-unquoted"), "{err:?}");
    assert!(err.to_string().contains("(the secret store's contents are not echoed)"), "{err}");
    let path = dir.path().join(SECRETS_RELPATH);
    assert_eq!(fs::read_to_string(path).unwrap(), original, "byte-identical");
}

/// The `config.toml` core once `[chat]` and `[wiki]` have left it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct CoreAfterMove {
    #[serde(default)]
    max_file_size: Option<u64>,
}

#[test]
fn without_their_owner_the_config_editor_keeps_the_reserved_tables_in_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let candidate = "max_file_size = 9\n\n[chat]\nmodel = \"m\"\n\n[wiki]\nmodel = \"w\"\n";
    write_config_validated(dir.path(), candidate, |text, path| {
        let routed = super::super::ownership::route_config::<CoreAfterMove>(
            text,
            path,
            &super::super::ownership::Claims::default(),
        )?;
        // `[chat]` is routed away and read by the core (S-643), `[wiki]` ignored.
        assert_eq!(routed.ignored, ["wiki"]);
        assert!(routed.reserved.contains_key("chat"));
        Ok(())
    })
    .expect("the reserved tables are tolerated");
    assert_eq!(
        fs::read_to_string(dir.path().join(CONFIG_RELPATH)).unwrap(),
        candidate,
        "the editor's write keeps them"
    );
}
