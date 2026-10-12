//! Who owns each top-level table of `config.toml` and each key of
//! `secrets.toml` — the core, or an extension ([FR-CF-09], CR-213).
//!
//! Every top-level key of `config.toml` is **routed** before the core parses
//! the file:
//!
//! 1. claimed by an active extension → validated by that extension
//!    ([`TableClaim`]), and never seen by the core's parse;
//! 2. one of the core's own keys → the core's `#[serde(deny_unknown_fields)]`
//!    parse, exactly as before;
//! 3. a [reserved table](RESERVED_TABLES) nobody claims → accepted, ignored and
//!    reported once as a notice, so a config written by a build with the
//!    extensions still parses in a build without them;
//! 4. anything else → left in the core's parse, which rejects it with the error
//!    text it always had.
//!
//! `secrets.toml` is routed the same way, per table: a table holding a key an
//! active extension claims ([`SecretClaim`]) is the extensions' (each of its
//! keys must be claimed), a core table is the core's, a reserved secret's table
//! nobody claims is ignored, and anything else is the core's to reject.
//!
//! Which keys the core owns is read off the core type's own serde field list,
//! never a hand-kept copy, so a table moving out of the core stops being
//! core-owned by construction: `[chat]` and `chat.api_key` did in [S-643], and
//! `[wiki]` in [S-644], whose fields the core now skips. The reserved lists are
//! core data, not an extension contribution: they must hold in a build that has
//! never seen the extension ([FR-CF-09] Notes).
//!
//! # Not owned, still read
//!
//! A reserved table's **value** comes back with every routed parse
//! ([`Routed::reserved`]), claimed or not, because the core still *reads* the
//! tables in [`CORE_READ_TABLES`] — `[chat]` and its key — after it stopped
//! *owning* them: they feed the `resolve_chat` seam both agents dial through, the
//! chat extension and the wiki generator, which inherits its provider and key
//! from `[chat]` ([ADR-42]). With the `chat` extension active the table is
//! validated by it; without it the core parses it exactly as before the move.
//!
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
//!
//! Extensions' claims reach this module once, from the composition root
//! ([`register_extension_config`]) — the same shape the telemetry
//! classification uses ([`crate::observability::register_extension_tools`]) —
//! so every config load in the process, on every surface, routes the same way
//! without each call site carrying the host.
//!
//! [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md
//! [S-643]: ../../../docs/planning/journal.md#s-643-chat-is-a-registered-extension
//! [S-644]: ../../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor,
};
use serde::Deserialize;

use super::error::ConfigError;
use super::{Config, Secrets};

/// The `config.toml` tables an extension owns once CR-213 has moved it out of
/// the core. A config carrying one of them parses in a build where no active
/// extension claims it: the table is ignored, reported once, and kept in the
/// file ([FR-CF-09]).
///
/// [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md
pub const RESERVED_TABLES: [&str; 2] = ["chat", "wiki"];

/// The `secrets.toml` keys, as `table.key`, an extension owns once CR-213
/// has moved it out of the core — tolerated like [`RESERVED_TABLES`], per
/// table.
pub const RESERVED_SECRETS: [&str; 1] = ["chat.api_key"];

/// The reserved tables the core no longer owns but still **reads** (S-643): the
/// `[chat]` policy and its secret are the `resolve_chat` seam both agents dial
/// through — the chat extension and the wiki generator, which inherits its
/// provider and key from `[chat]` ([ADR-42]). Claimed, such a table is validated
/// by its owner like any extension table; unclaimed, it is not ignored but parsed
/// by the core exactly as before it moved, so a build without the extension
/// reads, validates and reports it as it always did. Until the
/// chat configuration type moves out of the core, after which `[chat]` is
/// ignored like any reserved table.
///
/// [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
pub(crate) const CORE_READ_TABLES: [&str; 1] = ["chat"];

/// The value of a top-level `config.toml` table, as an extension validates it.
pub use toml::Value as TomlValue;

/// How an extension validates a table it owns: `Err` carries what is wrong,
/// and fails the load (or the editor's write) like a core validation error.
pub type TableValidator = Arc<dyn Fn(&TomlValue) -> Result<(), String> + Send + Sync>;

/// A top-level `config.toml` table an active extension owns, and how that
/// extension validates it.
#[derive(Clone)]
pub struct TableClaim {
    owner: &'static str,
    table: &'static str,
    validate: TableValidator,
}

impl TableClaim {
    /// `owner` (an extension id) owns `table`, validated by `validate`.
    pub fn new(
        owner: &'static str,
        table: &'static str,
        validate: impl Fn(&TomlValue) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            owner,
            table,
            validate: Arc::new(validate),
        }
    }

    /// The owning extension's id.
    pub fn owner(&self) -> &'static str {
        self.owner
    }

    /// The table's name.
    pub fn table(&self) -> &'static str {
        self.table
    }

    /// Validate `value` as this table, through its owner.
    ///
    /// # Errors
    /// What the owner finds wrong with it.
    pub fn validate(&self, value: &TomlValue) -> Result<(), String> {
        (self.validate)(value)
    }
}

impl fmt::Debug for TableClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableClaim")
            .field("owner", &self.owner)
            .field("table", &self.table)
            .finish_non_exhaustive()
    }
}

/// A `secrets.toml` key, as `table.key`, an active extension owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretClaim {
    /// The owning extension's id.
    pub owner: &'static str,
    /// The key, as `table.key`.
    pub key: &'static str,
}

/// The claims routing consults: the active extensions' tables and secret keys.
#[derive(Debug, Clone, Default)]
pub(crate) struct Claims {
    tables: Vec<TableClaim>,
    secrets: Vec<SecretClaim>,
}

impl Claims {
    /// Claims to route by — validated exactly as [`register_extension_config`]
    /// validates them, without touching the process registry.
    #[cfg(test)]
    pub(crate) fn new(tables: &[TableClaim], secrets: &[SecretClaim]) -> anyhow::Result<Self> {
        let mut claims = Self::default();
        claims.admit(tables, secrets)?;
        Ok(claims)
    }

    /// Add `tables` and `secrets`, or nothing when one is unacceptable.
    /// Re-adding a claim its owner already holds is a no-op.
    fn admit(&mut self, tables: &[TableClaim], secrets: &[SecretClaim]) -> anyhow::Result<()> {
        let core_tables = fields_of::<Config>();
        let core_secret_tables = fields_of::<Secrets>();
        let mut held: Vec<(&'static str, &'static str)> =
            self.tables.iter().map(|c| (c.table, c.owner)).collect();
        let mut fresh_tables = Vec::new();
        for claim in tables {
            if core_tables.contains(&claim.table) {
                anyhow::bail!(
                    "extension `{}` claims the config table `[{}]`, which the core owns",
                    claim.owner,
                    claim.table
                );
            }
            match held.iter().find(|(table, _)| *table == claim.table) {
                Some((_, owner)) if *owner == claim.owner => {}
                Some((_, owner)) => anyhow::bail!(
                    "the config table `[{}]` is claimed by both `{owner}` and `{}`",
                    claim.table,
                    claim.owner
                ),
                None => {
                    held.push((claim.table, claim.owner));
                    fresh_tables.push(claim.clone());
                }
            }
        }
        let mut held: Vec<SecretClaim> = self.secrets.clone();
        let mut fresh_secrets = Vec::new();
        for claim in secrets {
            let Some((table, key)) = claim.key.split_once('.') else {
                anyhow::bail!(
                    "extension `{}` claims the secret key `{}`, which is not `table.key`",
                    claim.owner,
                    claim.key
                );
            };
            if table.is_empty() || key.is_empty() || key.contains('.') {
                anyhow::bail!(
                    "extension `{}` claims the secret key `{}`, which is not `table.key`",
                    claim.owner,
                    claim.key
                );
            }
            if core_secret_tables.contains(&table) {
                anyhow::bail!(
                    "extension `{}` claims the secret key `{}`, whose table the core owns",
                    claim.owner,
                    claim.key
                );
            }
            match held.iter().find(|c| c.key == claim.key) {
                Some(earlier) if earlier.owner == claim.owner => {}
                Some(earlier) => anyhow::bail!(
                    "the secret key `{}` is claimed by both `{}` and `{}`",
                    claim.key,
                    earlier.owner,
                    claim.owner
                ),
                None => {
                    held.push(*claim);
                    fresh_secrets.push(*claim);
                }
            }
        }
        self.tables.extend(fresh_tables);
        self.secrets.extend(fresh_secrets);
        Ok(())
    }

    fn table(&self, name: &str) -> Option<&TableClaim> {
        self.tables.iter().find(|c| c.table == name)
    }

    /// Does an extension claim the `config.toml` table `name`?
    pub(crate) fn owns_table(&self, name: &str) -> bool {
        self.table(name).is_some()
    }

    /// Does an extension claim a key of the `secrets.toml` table `name`?
    pub(crate) fn owns_secret_table(&self, name: &str) -> bool {
        self.secret_keys_of(name).next().is_some()
    }

    fn secret_keys_of<'a>(&'a self, table: &'a str) -> impl Iterator<Item = &'a SecretClaim> + 'a {
        self.secrets
            .iter()
            .filter(move |c| c.key.split_once('.').is_some_and(|(t, _)| t == table))
    }
}

/// The claims registered in this process ([`register_extension_config`]).
static REGISTERED: RwLock<Claims> = RwLock::new(Claims {
    tables: Vec::new(),
    secrets: Vec::new(),
});

/// Register the active extensions' config tables and secret keys with this
/// process, so every `config.toml` / `secrets.toml` load routes them to their
/// owner ([FR-CF-09]). Called once, by the composition root, with the extension
/// host's active set.
///
/// Idempotent: re-registering a claim its owner already holds is a no-op.
/// Nothing is registered unless every claim is acceptable.
///
/// # Errors
/// A table the core owns, a secret key that is not `table.key` or whose table
/// the core owns, or a table or key already claimed by another extension.
///
/// [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md
pub fn register_extension_config(
    tables: &[TableClaim],
    secrets: &[SecretClaim],
) -> anyhow::Result<()> {
    REGISTERED
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .admit(tables, secrets)
}

/// The claims registered in this process.
pub(crate) fn registered() -> Claims {
    REGISTERED.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A routed parse: the core's value, the reserved tables it ignored, and the
/// values routed to extensions.
#[derive(Debug)]
pub(crate) struct Routed<T, X> {
    /// The core's parse, over the keys routed to it.
    pub value: T,
    /// The reserved tables no active extension claims, ignored.
    pub ignored: Vec<String>,
    /// What was routed to the extensions.
    pub extension: X,
    /// The value of every [reserved table](RESERVED_TABLES) (for `config.toml`)
    /// or reserved secret table (for `secrets.toml`) the document declares and
    /// the core does not own, claimed or ignored — what the core still reads of
    /// the tables it no longer owns (module docs, "Not owned, still read").
    pub reserved: toml::Table,
}

/// The values of the reserved tables among `document`'s keys that `core` does
/// not own.
fn reserved_values(document: &toml::Table, core: &[&str], reserved: &[&str]) -> toml::Table {
    document
        .iter()
        .filter(|(key, _)| reserved.contains(&key.as_str()) && !core.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Route `text` (a `config.toml`) and parse the core's share as `T`.
///
/// The core's parse sees the document **unchanged** whenever nothing is routed
/// away, so every error it raises — an unknown key, invalid TOML, a bad value —
/// is the error it always raised. When a key is routed away, the core's parse
/// still reads the original text and skips that key, so line and column stay
/// exact. Each extension table is validated after the core's parse succeeds, so
/// a core error is always reported first.
///
/// # Errors
/// [`ConfigError::Parse`] for the core's parse; [`ConfigError::InvalidValue`]
/// (`key` = the table) when an extension rejects its table — naming `path`, as a
/// parse error does, so a workspace tier and a member that both declare the table
/// are told apart.
pub(crate) fn route_config<T: DeserializeOwned>(
    text: &str,
    path: &Path,
    claims: &Claims,
) -> Result<Routed<T, ()>, ConfigError> {
    let parse_error = |source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    };
    // A document that is not TOML at all is the core parse's to report.
    let Ok(document) = toml::from_str::<toml::Table>(text) else {
        return toml::from_str::<T>(text)
            .map(|value| Routed {
                value,
                ignored: Vec::new(),
                extension: (),
                reserved: toml::Table::new(),
            })
            .map_err(parse_error);
    };
    let core = fields_of::<T>();
    let mut skip = BTreeSet::new();
    let mut owned = Vec::new();
    let mut ignored = Vec::new();
    for (key, value) in &document {
        if let Some(claim) = claims.table(key) {
            skip.insert(key.clone());
            owned.push((claim, value));
        } else if !core.contains(&key.as_str()) && RESERVED_TABLES.contains(&key.as_str()) {
            skip.insert(key.clone());
            if !CORE_READ_TABLES.contains(&key.as_str()) {
                ignored.push(key.clone());
            }
        }
    }
    let value = parse_skipping::<T>(text, &skip).map_err(parse_error)?;
    for (claim, table) in owned {
        claim.validate(table).map_err(|message| ConfigError::InvalidValue {
            key: claim.table.to_string(),
            message: format!(
                "{message} (the table belongs to the `{}` extension; in {})",
                claim.owner,
                path.display()
            ),
        })?;
    }
    Ok(Routed {
        value,
        ignored,
        extension: (),
        reserved: reserved_values(&document, core, &RESERVED_TABLES),
    })
}

/// Route `text` (a `secrets.toml`) and parse the core's share as `T`.
///
/// `redact` rebuilds a core parse error without any fragment of the file
/// ([NFR-SE-07]); the errors this function raises itself name a claimed key or
/// the table, never a value and never an unclaimed key's name. The extensions'
/// secrets come back as `table.key` → value.
///
/// # Errors
/// [`ConfigError::Parse`] (redacted) for the core's parse;
/// [`ConfigError::InvalidValue`] for an extension's table holding a key no
/// active extension claims, a key that is not a string, or a table that is not
/// a table.
///
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
pub(crate) fn route_secrets<T: DeserializeOwned>(
    text: &str,
    path: &Path,
    claims: &Claims,
    redact: impl Fn(&toml::de::Error) -> toml::de::Error,
) -> Result<Routed<T, BTreeMap<String, String>>, ConfigError> {
    let parse_error = |source: toml::de::Error| ConfigError::Parse {
        path: path.to_path_buf(),
        source: redact(&source),
    };
    let Ok(document) = toml::from_str::<toml::Table>(text) else {
        return toml::from_str::<T>(text)
            .map(|value| Routed {
                value,
                ignored: Vec::new(),
                extension: BTreeMap::new(),
                reserved: toml::Table::new(),
            })
            .map_err(parse_error);
    };
    let core = fields_of::<T>();
    let reserved_tables: BTreeSet<&str> =
        RESERVED_SECRETS.iter().filter_map(|k| k.split_once('.')).map(|(t, _)| t).collect();
    let mut skip = BTreeSet::new();
    let mut owned = Vec::new();
    let mut ignored = Vec::new();
    for (table, value) in &document {
        let claimed: Vec<&SecretClaim> = claims.secret_keys_of(table).collect();
        if !claimed.is_empty() {
            skip.insert(table.clone());
            owned.push((table, value, claimed));
        } else if !core.contains(&table.as_str()) && reserved_tables.contains(table.as_str()) {
            skip.insert(table.clone());
            if !CORE_READ_TABLES.contains(&table.as_str()) {
                ignored.push(table.clone());
            }
        }
    }
    let value = parse_skipping::<T>(text, &skip).map_err(parse_error)?;
    let mut extension = BTreeMap::new();
    for (table, value, claimed) in owned {
        let toml::Value::Table(keys) = value else {
            return Err(ConfigError::InvalidValue {
                key: table.clone(),
                message: format!("must be a table of secret keys (in {})", path.display()),
            });
        };
        for (key, secret) in keys {
            let full = format!("{table}.{key}");
            if !claimed.iter().any(|c| c.key == full) {
                // The key's own name is not echoed: a pasted secret can land in
                // the key position, which is why the core's redaction elides
                // every name outside its schema ([NFR-SE-07]).
                return Err(ConfigError::InvalidValue {
                    key: table.clone(),
                    message: format!(
                        "holds a key no active extension owns (the key is not echoed; in {})",
                        path.display()
                    ),
                });
            }
            let toml::Value::String(secret) = secret else {
                return Err(ConfigError::InvalidValue {
                    key: full,
                    message: format!("must be a string (the value is not echoed; in {})", path.display()),
                });
            };
            extension.insert(full, secret.clone());
        }
    }
    let reserved_tables: Vec<&str> = reserved_tables.into_iter().collect();
    Ok(Routed {
        value,
        ignored,
        extension,
        reserved: reserved_values(&document, core, &reserved_tables),
    })
}

/// Parse `text` as `T`, skipping the top-level keys in `skip`. With nothing to
/// skip this is exactly `toml::from_str`.
fn parse_skipping<T: DeserializeOwned>(
    text: &str,
    skip: &BTreeSet<String>,
) -> Result<T, toml::de::Error> {
    if skip.is_empty() {
        return toml::from_str(text);
    }
    T::deserialize(Skipping {
        inner: toml::Deserializer::new(text),
        skip,
    })
}

/// The ignored reserved tables already reported in this process.
static REPORTED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Report the reserved tables `file` declares and no active extension claims —
/// once per table per process, as one notice ([FR-CF-09]). A table reported
/// for `config.toml` is not reported again for `secrets.toml`: it is the same
/// missing extension.
///
/// [FR-CF-09]: ../../../docs/specs/requirements/FR-CF-09.md
pub(crate) fn report_ignored(file: &Path, ignored: &[String]) {
    report_ignored_in(&REPORTED, file, ignored);
}

/// [`report_ignored`] against `reported` — the process's set, or a test's own.
fn report_ignored_in(reported: &Mutex<BTreeSet<String>>, file: &Path, ignored: &[String]) {
    if let Some(notice) = first_notice(reported, file, ignored) {
        tracing::warn!("{notice}");
    }
}

/// The notice for the tables of `ignored` not yet in `reported` (recording
/// them), or `None` when every one was reported before.
fn first_notice(reported: &Mutex<BTreeSet<String>>, file: &Path, ignored: &[String]) -> Option<String> {
    let mut reported = reported.lock().unwrap_or_else(PoisonError::into_inner);
    let fresh: Vec<String> = ignored
        .iter()
        .filter(|table| reported.insert((*table).clone()))
        .map(|table| format!("[{table}]"))
        .collect();
    (!fresh.is_empty()).then(|| {
        format!(
            "notice: {} declares {}, which no active extension claims: ignored, and kept in the file",
            file.display(),
            fresh.join(", ")
        )
    })
}

/// The field names a `#[derive(Deserialize)]` struct `T` declares — the keys
/// its `deny_unknown_fields` parse accepts. Empty for a type that is not a
/// struct.
pub(crate) fn fields_of<T: DeserializeOwned>() -> &'static [&'static str] {
    match T::deserialize(FieldsProbe) {
        Err(Found(fields)) => fields,
        Ok(_) => &[],
    }
}

/// A deserializer that answers nothing and reports the fields it is asked for.
struct FieldsProbe;

/// [`FieldsProbe`]'s only outcome.
#[derive(Debug)]
struct Found(&'static [&'static str]);

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fields {:?}", self.0)
    }
}

impl std::error::Error for Found {}

impl de::Error for Found {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Found(&[])
    }
}

impl<'de> Deserializer<'de> for FieldsProbe {
    type Error = Found;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Found> {
        Err(Found(&[]))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Found> {
        Err(Found(fields))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// A deserializer that hides the top-level keys in `skip` from the type it
/// deserializes. Every other key — its name, its value and any error either
/// raises — goes through `inner` untouched, inside `inner`'s own key and value
/// handling, so the error carries the position and text it always had.
struct Skipping<'s, D> {
    inner: D,
    skip: &'s BTreeSet<String>,
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Skipping<'_, D> {
    type Error = D::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        self.inner.deserialize_any(SkippingVisitor {
            inner: visitor,
            skip: self.skip,
        })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.inner.deserialize_struct(
            name,
            fields,
            SkippingVisitor {
                inner: visitor,
                skip: self.skip,
            },
        )
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

struct SkippingVisitor<'s, V> {
    inner: V,
    skip: &'s BTreeSet<String>,
}

impl<'de, V: Visitor<'de>> Visitor<'de> for SkippingVisitor<'_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(f)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        self.inner.visit_map(SkippingMap {
            inner: map,
            skip: self.skip,
        })
    }
}

struct SkippingMap<'s, A> {
    inner: A,
    skip: &'s BTreeSet<String>,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for SkippingMap<'_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        let mut seed = seed;
        loop {
            let key = KeyOrSkip {
                seed,
                skip: self.skip,
            };
            match self.inner.next_key_seed(key)? {
                None => return Ok(None),
                Some(Ok(key)) => return Ok(Some(key)),
                Some(Err(returned)) => {
                    self.inner.next_value::<IgnoredAny>()?;
                    seed = returned;
                }
            }
        }
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        self.inner.next_value_seed(seed)
    }
}

/// A key seed that hands the type's own seed back, unused, for a skipped key —
/// and otherwise runs it inside the inner map's key handling.
struct KeyOrSkip<'s, K> {
    seed: K,
    skip: &'s BTreeSet<String>,
}

impl<'de, K: DeserializeSeed<'de>> DeserializeSeed<'de> for KeyOrSkip<'_, K> {
    type Value = Result<K::Value, K>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        let key = String::deserialize(deserializer)?;
        if self.skip.contains(&key) {
            return Ok(Err(self.seed));
        }
        self.seed
            .deserialize(de::value::StringDeserializer::<D::Error>::new(key))
            .map(Ok)
    }
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;
