//! The gitignored `.logos/secrets.toml` key store ([FR-CF-06], [NFR-SE-07],
//! [ADR-40]).
//!
//! This is the **first secret Logos stores**. Unlike the checked-in policy files
//! (`config.toml`/`rules.toml`) it is **gitignored** — it does not travel into
//! worktrees ([`logos init`](crate::init) adds it to `.gitignore`) — and it
//! holds **only** the chat API key, never any non-secret policy.
//!
//! # The never-echo invariant ([NFR-SE-07])
//! The raw key is loadable here (the agent needs it at egress time) but must
//! **never** surface in an HTTP response body, a log line, or a rendered page.
//! Two guards enforce that structurally:
//!
//! 1. [`Secrets`] and [`ChatSecrets`] carry **hand-written `Debug`** impls that
//!    render presence + last-4 only, so a `{:?}` in a `tracing` event or an
//!    `anyhow` context can never leak the key (the same defense `agent-core`'s
//!    `ProviderConfig` uses).
//! 2. The read-model the editor renders is the [`MaskedSecret`] — presence +
//!    last-4, `Serialize`-safe — **not** the raw [`Secrets`]. The raw key is
//!    never placed in a serialisable read-model, so it cannot be JSON-encoded
//!    into a response by construction.
//!
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
//! [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md

use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::ConfigError;

/// The conventional location of the gitignored secret store within a root.
pub(crate) const SECRETS_RELPATH: &str = ".logos/secrets.toml";

/// How many trailing characters of a key the masked view exposes ([FR-CF-06]:
/// "presence + last-4").
const LAST_N: usize = 4;

/// The parsed `.logos/secrets.toml` — the gitignored secret store ([FR-CF-06]).
///
/// Mirrors `config.toml`'s `[chat]` table shape (one `[chat]` table) so the
/// key sits beside its policy conceptually while living in a separate,
/// gitignored file. `#[serde(deny_unknown_fields)]` keeps a typo'd secret key
/// loud rather than silently dropped.
///
/// The derived `Serialize`/`Deserialize` are used only for at-rest TOML I/O
/// (the gitignored file legitimately holds the raw key); the **`Debug` is
/// hand-written** so no diagnostic path leaks it ([NFR-SE-07]).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Secrets {
    /// The chat secrets (`[chat]`).
    #[serde(default)]
    pub chat: ChatSecrets,
}

/// The `[chat]` table of `secrets.toml`: the API key only ([FR-CF-06]).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSecrets {
    /// The chat API key. `None` when unset (no `[chat]` table, or no `api_key`).
    ///
    /// **`pub(crate)`, not `pub`** ([NFR-SE-07], defense-in-depth): the raw key
    /// must never leave `logos-core` by a direct field read. Outside the crate
    /// the key is reachable only through [`Secrets::chat_api_key`] (the trimmed
    /// value, for the agent that dials) or [`Secrets::chat_key_masked`] (the
    /// presence + last-4 the surface renders) — never the bare field. The
    /// derived `Serialize` exists for the at-rest TOML write only; the type is
    /// never placed in a surface read-model (those carry [`MaskedSecret`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_key: Option<String>,
}

impl Secrets {
    /// The chat API key, if present and non-empty.
    ///
    /// A present-but-blank `api_key` is treated as unset — the same shape a
    /// "clear the key" write produces — so a caller never dials with an empty
    /// credential.
    pub fn chat_api_key(&self) -> Option<&str> {
        self.chat
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }

    /// The masked view of the chat key for the Config editor ([FR-CF-06],
    /// [NFR-SE-07]): presence + last-4, never the raw value.
    pub fn chat_key_masked(&self) -> MaskedSecret {
        MaskedSecret::from_key(self.chat_api_key())
    }
}

/// `Debug` that **redacts** the key ([NFR-SE-07]) — presence + last-4 only.
impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secrets").field("chat", &self.chat).finish()
    }
}

/// `Debug` that **redacts** the key ([NFR-SE-07]) — masks the raw stored value
/// (no trim/empty filtering) before it is ever rendered.
impl fmt::Debug for ChatSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatSecrets")
            .field("api_key", &MaskedSecret::from_key(self.api_key.as_deref()))
            .finish()
    }
}

/// The masked view of a secret the editor renders and the surface serialises:
/// **presence + last-4 only** ([FR-CF-06], [NFR-SE-07]). The raw key is never a
/// field here, so it can never be JSON-encoded into a response by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MaskedSecret {
    /// Whether a (non-empty) key is set.
    pub present: bool,
    /// The last ≤4 characters of the key when present, for recognisability
    /// ([FR-CF-06]); `None` when absent or shorter than would be meaningful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last4: Option<String>,
}

impl MaskedSecret {
    /// Mask a key into presence + last-4 ([FR-CF-06]).
    ///
    /// `None`/empty ⇒ absent. A present key exposes its last ≤4 characters; a key
    /// of fewer than 4 chars exposes only what it has (still ≤4), which is all
    /// the recognisability there is — it never pads or fabricates.
    pub fn from_key(key: Option<&str>) -> Self {
        match key.filter(|k| !k.is_empty()) {
            None => MaskedSecret {
                present: false,
                last4: None,
            },
            Some(k) => {
                // Take the last ≤4 *characters* (not bytes) so a multi-byte key
                // never panics on a non-char-boundary slice.
                let reversed_tail: String = k.chars().rev().take(LAST_N).collect();
                let last4: String = reversed_tail.chars().rev().collect();
                MaskedSecret {
                    present: true,
                    last4: Some(last4),
                }
            }
        }
    }
}

/// Parse a `secrets.toml` from already-read `text`; `path` is for error
/// attribution only. The single-read seam mirroring [`super::parse_config`].
///
/// # Errors
/// [`ConfigError::Parse`] on invalid TOML or an unknown key
/// (`#[serde(deny_unknown_fields)]`).
pub(crate) fn parse_secrets(text: &str, path: &Path) -> Result<Secrets, ConfigError> {
    toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Load `secrets.toml` from `<root>/.logos/secrets.toml`, or
/// [`Secrets::default`] (no key) if it is absent ([FR-CF-06], [NFR-DM-04]).
///
/// An absent secret store is **not** a fault — like the policy files, a worktree
/// need not carry one (and being gitignored, it usually won't). A present store
/// that does not parse fails loud through [`parse_secrets`].
///
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
pub fn load_secrets_from_root(root: &Path) -> Result<Secrets, ConfigError> {
    let path = root.join(SECRETS_RELPATH);
    match fs::read_to_string(&path) {
        Ok(text) => parse_secrets(&text, &path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Secrets::default()),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_store_loads_as_no_key() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = load_secrets_from_root(dir.path()).unwrap();
        assert_eq!(secrets, Secrets::default());
        assert!(secrets.chat_api_key().is_none());
        let masked = secrets.chat_key_masked();
        assert!(!masked.present);
        assert!(masked.last4.is_none(), "an absent key has no last-4");
    }

    #[test]
    fn present_store_loads_the_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".logos")).unwrap();
        fs::write(
            dir.path().join(SECRETS_RELPATH),
            "[chat]\napi_key = \"sk-secret-abcd1234\"\n",
        )
        .unwrap();
        let secrets = load_secrets_from_root(dir.path()).unwrap();
        assert_eq!(secrets.chat_api_key(), Some("sk-secret-abcd1234"));
    }

    #[test]
    fn blank_key_is_treated_as_unset() {
        let mut s = Secrets::default();
        s.chat.api_key = Some("   ".to_string());
        assert!(s.chat_api_key().is_none(), "a blank key is unset");
        assert!(!s.chat_key_masked().present);
    }

    #[test]
    fn unknown_secret_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".logos")).unwrap();
        fs::write(
            dir.path().join(SECRETS_RELPATH),
            "[chat]\napi_key = \"k\"\nbogus = 1\n",
        )
        .unwrap();
        let err = load_secrets_from_root(dir.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn masked_secret_exposes_only_presence_and_last4() {
        let masked = MaskedSecret::from_key(Some("sk-secret-abcd1234"));
        assert!(masked.present);
        assert_eq!(masked.last4.as_deref(), Some("1234"));

        // Absent ⇒ no last4.
        let none = MaskedSecret::from_key(None);
        assert!(!none.present);
        assert!(none.last4.is_none());

        // Empty ⇒ absent.
        assert!(!MaskedSecret::from_key(Some("")).present);

        // Short key exposes only what it has (≤4), never pads.
        let short = MaskedSecret::from_key(Some("ab"));
        assert!(short.present);
        assert_eq!(short.last4.as_deref(), Some("ab"));
    }

    #[test]
    fn masked_last4_boundary_lengths() {
        // The ≤4 boundary: 3 chars → all 3; exactly 4 → the whole key; 5 → the
        // last 4 (first char suppressed). Guards the `take(4)` edge precisely.
        assert_eq!(
            MaskedSecret::from_key(Some("abc")).last4.as_deref(),
            Some("abc")
        );
        assert_eq!(
            MaskedSecret::from_key(Some("abcd")).last4.as_deref(),
            Some("abcd"),
            "a 4-char key exposes the whole key (== last 4)"
        );
        assert_eq!(
            MaskedSecret::from_key(Some("abcde")).last4.as_deref(),
            Some("bcde"),
            "a 5-char key suppresses all but the last 4"
        );
    }

    #[test]
    fn masked_last4_is_char_safe_for_multibyte_keys() {
        // A non-ASCII tail must not panic on a byte-slice boundary.
        let masked = MaskedSecret::from_key(Some("key-café"));
        assert!(masked.present);
        assert_eq!(masked.last4.as_deref(), Some("café"));
    }

    /// [NFR-SE-07]: neither the raw key nor any `Debug` rendering leaks it. The
    /// `Debug` of `Secrets`/`ChatSecrets` shows presence + last-4 only, and the
    /// `MaskedSecret` serialises without the raw value.
    #[test]
    fn debug_and_masked_never_leak_the_raw_key() {
        let mut s = Secrets::default();
        s.chat.api_key = Some("sk-secret-DEADBEEF".to_string());

        let dbg = format!("{s:?}");
        assert!(
            !dbg.contains("sk-secret-DEADBEEF"),
            "Debug must never render the raw key (NFR-SE-07): {dbg}"
        );
        assert!(dbg.contains("BEEF"), "Debug shows last-4 for diagnosability: {dbg}");

        let json = serde_json::to_string(&s.chat_key_masked()).unwrap();
        assert!(
            !json.contains("sk-secret-DEADBEEF"),
            "the masked read-model never serialises the raw key: {json}"
        );
        assert!(json.contains("BEEF"), "masked JSON carries last-4: {json}");
        assert!(json.contains("\"present\":true"));
    }
}
