//! Secret masking for the scanner text Logos stores or shows ([CR-217],
//! [FR-SC-03], [BR-67]).
//!
//! Two kinds of scanner text pass through here:
//! - **Every finding message, before the write.** A secret-class finding's
//!   message is replaced whole ([`super::redact`]); every other message is
//!   stored masked, so a rule that echoes a matched value in an ordinary
//!   message persists no secret either.
//! - **A failed scanner's output.** A scanner can echo what it matched: a secret
//!   scanner prints the line, a dependency scanner prints a registry URL with
//!   its credentials. The whole bounded text goes to a log file under
//!   [`OUTPUT_DIR`](crate::security::OUTPUT_DIR); the payload carries only a
//!   short [`excerpt`], masked here.
//!
//! The masker leans wide: a masked word that was harmless costs a word of the
//! text (the failure log still holds it); an unmasked credential is a leak.
//!
//! Masked, each replaced by [`MASK`]:
//! - a PEM or PGP private-key block, from its `BEGIN` line to its `END` line
//!   (or the end of the text);
//! - the user-info of a URL (`scheme://user:password@host`, or a bare
//!   `scheme://token@host`), up to the last `@` of the word, so a password
//!   holding an `@` or a `/` is masked whole;
//! - an AWS access key id (`AKIA…`, `ASIA…`), which may hold no digit;
//! - the value after a credential-named key (`password=…`, `DB_PASS=…`,
//!   `api_key: …`, `"Authorization": "Bearer …"`, `Authorization: token …`,
//!   `Cookie: …`, `the password is …`), with any of the separators `=`, `:`,
//!   `=>`, `:=`, `==`, also after up to three words on the key's line
//!   (`password for db: …`, the first word masked too, as it is after a bare
//!   key) unless a `.`, `,` or `;` ends one first, and the key and value bare,
//!   quoted or escape-quoted
//!   (`\"token\": \"…\"`). A quoted value is masked to its closing quote,
//!   spaces and all; an unquoted one to the next space, quote or comma, so a
//!   bare value's later words stay (the log keeps the whole text). The short key words (`pass`, `pw`, `pwd`,
//!   `auth`) count only as a whole segment of a key (`DB_PASS`, `x-auth`), so
//!   `passed` and `author` are not keys;
//! - the credential after a bare `Bearer` or `Basic`, key or no key;
//! - any word of 20 or more token characters that mixes letters and digits —
//!   the shape of a generated key or token, whatever its provider — except in a
//!   package coordinate (`github.com/xuri/excelize/v2@2.10.1`,
//!   `@scope/pkg-name1234567890@1.2.3`), which reads whole: a name, `@`, then a
//!   version of two or more numeric parts; or a module path with no version
//!   (`github.com/xuri/excelize/v2`), a dot-qualified host and `/` segments. A
//!   word in a coordinate's name is left alone only when none of its `/` segments
//!   is itself token-shaped, so `<token>@1.2.3` and `host.tld/<token>` are
//!   masked; a coordinate after `://` is a URL's path and is masked as before.
//!   A coordinate's user-info is masked by the URL rule first.
//!
//! [CR-217]: ../../../../docs/requests/CR-217-security-findings-in-graph-context.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [BR-67]: ../../../../docs/specs/software-spec.md#328-security-findings

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

/// What a masked span is replaced with.
pub const MASK: &str = "[masked]";

/// At most this many trailing lines of a failure reach a payload.
const EXCERPT_LINES: usize = 12;

/// At most this many bytes of a failure reach a payload.
const EXCERPT_BYTES: usize = 1024;

static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)")
        .expect("the PEM pattern compiles")
});

static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)([a-z][a-z0-9+.\-]*://)(?:[^\s/:@]*:[^\s]*|[^\s/]+)@").expect("the URL pattern compiles")
});

static AWS_KEY_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:AKIA|ASIA|ABIA|ACCA|A3T[A-Z0-9])[A-Z0-9]{16}\b").expect("the AWS pattern compiles")
});

/// A credential-named key, bare, quoted or escape-quoted.
const KEY: &str = r#"(?i)\b(?:[a-z0-9_\-.]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|client[_-]?key|credential|authorization|cookie|session[_-]?id)[a-z0-9_\-]*|(?:[a-z0-9]+[_\-.])*(?:pass|pw|pwd|auth)(?:[_\-.][a-z0-9]+)*\b)\\?["']?"#;

/// An optional auth scheme, then the value: quoted to its closing quote, or
/// unquoted (the last group of each pattern below).
const SCHEME_VALUE: &str = r#"\s*(?:\\?["']?\s*(?:bearer|basic|token|digest)\s+)?(\\"[^"\\\n]*(?:\\")?|"(?:[^"\\\n]|\\.)*"?|'[^'\n]*'?|[^\s"',\\]+)"#;

/// A word between a key and its separator: no separator in it, and no `,` or
/// `;` in it or `.` at its end, which close a clause.
const FILLER_WORD: &str = r"[^\s:=,;]*[^\s:=,;.]";

/// A key, its separator (or `is`, or a space), then the value.
static CREDENTIAL_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&[KEY, r"\s*(?:=>|:=|==|[:=]|\s+is\s+|\s+)", SCHEME_VALUE].concat())
        .expect("the credential pattern compiles")
});

/// A key, up to three words on its line, a separator, then the value (group 2).
/// The first word (group 1) is masked too, as [`CREDENTIAL_VALUE`] masks it
/// after a bare key, and also where that rule took this key as another's value.
static FILLER_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &[KEY, r"[ \t]+(", FILLER_WORD, r")(?:[ \t]+", FILLER_WORD, r"){0,2}[ \t]*(?:=>|:=|==|[:=])", SCHEME_VALUE]
            .concat(),
    )
    .expect("the filler pattern compiles")
});

static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(\b(?:bearer|basic)\s+)([^\s"',]+)"#).expect("the bearer pattern compiles")
});

static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_\-+/=]{20,}").expect("the token pattern compiles"));

/// A package coordinate: a name (a Go module path, an npm name, scoped or not),
/// `@`, then a version (group `version`) of two or more numeric parts (`2.10.1`,
/// `v0.0.0-2021…`, `1.2.3-rc.1`, `v2.0.0+incompatible`); or a module path with no
/// version, a dot-qualified host and `/` segments. A match only says where a
/// coordinate may lie: [`Coordinate::holds`] says whether a word in it is one.
static COORDINATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&[
        r"[A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)*",
        r"@(?P<version>v?[0-9]+(?:\.[0-9]+)+(?:[.\-+][A-Za-z0-9]+)*)",
        r"|(?:[A-Za-z0-9\-]+\.)+[A-Za-z]{2,}(?:/[A-Za-z0-9_.\-]+)+",
    ]
    .concat())
    .expect("the coordinate pattern compiles")
});

/// Where a [`COORDINATE`] match lies, and where its version does.
struct Coordinate {
    span: Range<usize>,
    version: Option<Range<usize>>,
}

impl Coordinate {
    /// Whether `word`, a token-shaped word, reads whole here: it lies in the
    /// coordinate, which does not follow `://` (a URL's path is not a package
    /// path), and it is either in the version (a pseudo-version's commit stamp is
    /// a word of its own) or made of name-like segments — none of its `/`
    /// segments is itself token-shaped, so a `<token>@1.2.3`, a `host.tld/<token>`
    /// and a webhook's path are still masked.
    fn holds(&self, text: &str, word: &regex::Match<'_>) -> bool {
        self.span.start <= word.start()
            && !text[..self.span.start].ends_with("://")
            && (self.version.as_ref().is_some_and(|v| v.start <= word.start())
                || word.as_str().split('/').all(|segment| !token_shaped(segment)))
    }
}

/// 20 or more token characters mixing letters and digits: the shape of a
/// generated key or token, whatever its provider.
fn token_shaped(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 20 && bytes.iter().any(u8::is_ascii_digit) && bytes.iter().any(u8::is_ascii_alphabetic)
}

/// `text` with every span the [module docs](self) list replaced by [`MASK`].
pub fn mask_secrets(text: &str) -> String {
    let text = PEM.replace_all(text, MASK);
    let text = URL_USERINFO.replace_all(&text, format!("${{1}}{MASK}@").as_str());
    let text = AWS_KEY_ID.replace_all(&text, MASK);
    let text = mask_credential_values(&text);
    let text = BEARER.replace_all(&text, format!("${{1}}{MASK}").as_str());
    mask_tokens(&text)
}

/// `text` with every token-shaped word masked, bar one a [`Coordinate`] holds.
fn mask_tokens(text: &str) -> String {
    let coordinates: Vec<Coordinate> = COORDINATE
        .captures_iter(text)
        .map(|caps| Coordinate {
            span: caps.get(0).expect("a match has a span").range(),
            version: caps.name("version").map(|m| m.range()),
        })
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for word in TOKEN.find_iter(text) {
        // The coordinates are sorted and disjoint: the first one that reaches the
        // word's end is the only one that can hold it.
        let held = coordinates
            .get(coordinates.partition_point(|c| c.span.end < word.end()))
            .is_some_and(|c| c.holds(text, &word));
        if token_shaped(word.as_str()) && !held {
            out.push_str(&text[at..word.start()]);
            out.push_str(MASK);
            at = word.end();
        }
    }
    out.push_str(&text[at..]);
    out
}

/// `text` with the spans both credential patterns find in it masked. Both run
/// over the same text and their spans are joined, so one never hides a key
/// from the other; spans that overlap are masked as one.
fn mask_credential_values(text: &str) -> String {
    let mut spans: Vec<(Range<usize>, String)> = Vec::new();
    for caps in CREDENTIAL_VALUE.captures_iter(text) {
        let value = caps.get(1).expect("the value group always takes part");
        spans.push((value.range(), masked_value(value.as_str())));
    }
    for caps in FILLER_VALUE.captures_iter(text) {
        let word = caps.get(1).expect("the first word always takes part");
        let value = caps.get(2).expect("the value group always takes part");
        spans.push((word.range(), MASK.to_string()));
        spans.push((value.range(), masked_value(value.as_str())));
    }
    spans.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, masked) in spans {
        if range.start < at {
            at = at.max(range.end);
            continue;
        }
        out.push_str(&text[at..range.start]);
        out.push_str(&masked);
        at = range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A value masked. A quoted value keeps its quotes, so the surrounding text
/// still reads; an escaped-quoted one (`\"…\"`, JSON inside a JSON string) too.
fn masked_value(value: &str) -> String {
    let quote = if value.starts_with("\\\"") {
        Some("\\\"".to_string())
    } else {
        value.chars().next().filter(|c| *c == '"' || *c == '\'').map(String::from)
    };
    let (open, close) = quote.map_or((String::new(), String::new()), |q| (q.clone(), q));
    format!("{open}{MASK}{close}")
}

/// The payload's view of a failure's text: its last [`EXCERPT_LINES`] lines,
/// at most [`EXCERPT_BYTES`] bytes of them, masked. Masking runs over the whole
/// text first, so a secret that straddles the cut is masked before it is cut.
pub fn excerpt(text: &str) -> String {
    let masked = mask_secrets(text.trim());
    let lines: Vec<&str> = masked.lines().collect();
    let tail = lines[lines.len().saturating_sub(EXCERPT_LINES)..].join("\n");
    if tail.len() <= EXCERPT_BYTES {
        return tail;
    }
    let mut start = tail.len() - EXCERPT_BYTES;
    while !tail.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &tail[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each shape is masked, and the words around it survive.
    #[test]
    fn each_secret_shape_is_masked_and_its_context_kept() {
        let cases = [
            ("error: password=hunter2 rejected", "hunter2", "rejected"),
            ("api_key: \"s3cr3t-value\"", "s3cr3t-value", "api_key"),
            ("Authorization: Bearer abc.def", "abc.def", "Authorization"),
            ("fetch https://deploy:pa55word@registry.example/x failed", "pa55word", "registry.example"),
            ("found ghp_A1b2C3d4E5f6G7h8I9j0K1l2 in config", "ghp_A1b2C3d4E5f6G7h8I9j0K1l2", "in config"),
            (
                "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----\ndone",
                "MIIEow",
                "done",
            ),
            ("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1r (truncated", "b3BlbnNzaC1r", "[masked]"),
            // The shapes the S-667 review found past the first version.
            ("error: password: 'correct horse battery staple'", "horse battery staple", "error"),
            ("{\"password\":\"hunter two\"}", "hunter two", "password"),
            ("{\"Authorization\":\"Bearer abc.def\"}", "abc.def", "Bearer"),
            ("{\"Authorization\": \"Basic dXNlcjpwYXNz\"}", "dXNlcjpwYXNz", "Basic"),
            ("Authorization: \"Bearer abc.def\"", "abc.def", "Bearer"),
            ("line 4: DB_PASS=Sup3rS3cret!", "Sup3rS3cret", "line 4"),
            ("found AKIAQWERTYUIOPASDFGH in config.py:3", "AKIAQWERTYUIOPASDFGH", "config.py:3"),
            ("upstream 401 for Bearer s3cr3tTok", "s3cr3tTok", "upstream 401"),
            ("password=s3cr;tPart rejected", "tPart", "rejected"),
            ("https://deploy:p@ssw0rd@registry.example/x", "ssw0rd", "registry.example"),
            ("the password is hunter2", "hunter2", "the password"),
            ("x-auth: abc123", "abc123", "x-auth"),
            ("client_secret=abcd", "abcd", "client_secret"),
            ("smtp_pw: Zx81q", "Zx81q", "smtp_pw"),
            // Exactly the token rule's 20-character floor.
            ("leaked Abc123Def456Ghi789Jk here", "Abc123Def456Ghi789Jk", "leaked"),
            // The shapes the sprint review found past the S-667 review.
            ("'password' => 'PhEarrow' in config.php", "PhEarrow", "config.php"),
            ("DB_PASSWORD := MkEcolon", "MkEcolon", "DB_PASSWORD"),
            ("if password == \"EeEcompare\":", "EeEcompare", "if password"),
            ("https://bob:SlEa/SmEb9@host.example/y", "SmEb9", "host.example"),
            ("https://ghpTokenOnly@github.com/o/r", "ghpTokenOnly", "github.com"),
            (r#"{"password":"JrE\"tail"}"#, "tail", "password"),
            (r#"body: {\"token\": \"TkPjsonval9\"}"#, "TkPjsonval9", "body"),
            ("Authorization: token GtEshort1", "GtEshort1", "Authorization"),
            ("Cookie: sessionid=CkEsess1; path=/", "CkEsess1", "Cookie"),
            ("client_key=CkEkey22", "CkEkey22", "client_key"),
            (
                "-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBGdE\n-----END PGP PRIVATE KEY BLOCK-----\nok",
                "lQOYBGdE",
                "ok",
            ),
        ];
        for (text, secret, kept) in cases {
            let masked = mask_secrets(text);
            assert!(!masked.contains(secret), "{secret:?} survives in {masked:?}");
            assert!(masked.contains(kept), "{kept:?} lost from {masked:?}");
            assert!(masked.contains(MASK), "{masked:?}");
        }
    }

    /// The near misses stay: a long word without a digit, a short mixed word,
    /// and a version string are not secrets.
    #[test]
    fn ordinary_scanner_text_is_left_alone() {
        for text in [
            "semgrep 1.85.0: could not find the configuration file",
            "the vulnerability database is unavailable offline",
            "exit status 2 after 13 rules",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            // `pass` and `auth` are keys only as a whole key segment.
            "12 tests passed, 1 failed",
            "the author field is required",
            "compass bypass: 3",
            // A `:` or `=` with no credential key in reach is not a value.
            "error in config.yaml: unexpected key",
            "rule python.flask.debug: debug mode enabled",
            "3 issues found: see the report",
            "retries=3 timeout=30s",
        ] {
            assert_eq!(mask_secrets(text), text);
        }
    }

    /// A key, up to three words, then `:` or `=`: the value is masked, and the
    /// first word as after a bare key. The recorded line is the sprint 106
    /// review's own output, so the masker no longer lets it through either.
    #[test]
    fn a_value_after_filler_words_is_masked() {
        for (text, expected) in [
            ("password for db: Tr0ub4dorNeedle&3", "password [masked] db: [masked]"),
            ("password [masked] db: Tr0ub4dorNeedle&3", "password [masked] db: [masked]"),
            ("api_key for staging = Zq9vFiller7", "api_key [masked] staging = [masked]"),
            ("the secret of the app: 'Swordfish Filler'", "the secret [masked] the app: '[masked]'"),
            ("DB_PASSWORD (prod): Pr0dFiller", "DB_PASSWORD [masked] [masked]"),
            ("token from vault => TkFiller5", "token [masked] vault => [masked]"),
            ("api_key for staging := ColEq9x", "api_key [masked] staging := [masked]"),
            ("password for db == EqEq9x", "password [masked] db == [masked]"),
            ("password\tfor\tdb:\tTabS3cret9", "password\t[masked]\tdb:\t[masked]"),
            ("password for db\t: TabSep9", "password [masked] db\t: [masked]"),
            // A `.` inside a word does not end the clause.
            ("password for db.prod: DotIn9x", "password [masked] db.prod: [masked]"),
            // The first separator ends the words: its value is masked, not a later one's.
            ("password for db: Hun7erCol: rejected", "password [masked] db: [masked] rejected"),
        ] {
            assert_eq!(mask_secrets(text), expected, "{text:?}");
        }
    }

    /// The word right after a key is masked as it was before the filler rule,
    /// a `:` or `=` further on notwithstanding: it is where a scanner echoes the
    /// value. A key among the words, or as the value, still masks its own.
    #[test]
    fn the_word_after_a_key_stays_masked_beside_a_later_separator() {
        for (text, secret) in [
            ("found password Hunter22 at config.yml:5", "Hunter22"),
            ("password hunter2 in app.py:3", "hunter2"),
            ("npm ERR! auth token abc123 invalid: 401", "abc123"),
            ("the password is hunter2: rotate it", "hunter2"),
            ("Hardcoded password \"hunter2\" at line: 4", "hunter2"),
            ("Hardcoded password 'hunter2' in settings.py: 4", "hunter2"),
            ("api_key abc123 token=xyz", "abc123"),
            ("Cookie sid123 path=/", "sid123"),
            ("password for token abc: x", "abc"),
            ("password for db: token abc", "abc"),
            // `password` is `token`'s value here, so only the filler rule sees it as a key.
            ("token password hunter2 in app.py:3", "hunter2"),
        ] {
            let masked = mask_secrets(text);
            assert!(!masked.contains(secret), "{secret:?} survives in {masked:?}");
        }
    }

    /// The reach stops at three words, at a sentence's end and at a line's: a
    /// value past it is prose, not the key's.
    #[test]
    fn a_value_past_the_keys_reach_stays() {
        for (text, kept) in [
            ("password must be rotated every quarter: see docs", "see docs"),
            // Exactly four words: one past the reach.
            ("password must be rotated quarterly: see docs", "see docs"),
            ("token expired. Retry at: noon", "noon"),
            ("token expired, retry at: noon", "noon"),
            ("token expired; retry at: noon", "noon"),
            ("password\nnext line: kept", "kept"),
            // An indented next line is still another line.
            ("password\n  next line: kept", "kept"),
            ("token\n\tat com.Foo.bar(Foo.java:12)", "Foo.java:12"),
        ] {
            let masked = mask_secrets(text);
            assert!(masked.contains(kept), "{kept:?} lost from {masked:?}");
        }
    }

    /// A package coordinate (`name@version`) reads whole: its path or its scoped
    /// name can be 20 characters of letters and digits, which is not a token.
    #[test]
    fn a_package_coordinate_is_never_masked_as_a_token() {
        for text in [
            "Package 'github.com/xuri/excelize/v2@2.10.1' is vulnerable",
            "Package '@scope/pkg-name1234567890@1.2.3' is vulnerable",
            // A Go pseudo-version: its commit stamp is a long mixed word of its own.
            "Package 'github.com/o/r@v0.0.0-20210101123456-abcdef123456' is vulnerable",
            "Package 'github.com/o/r/v2@v2.0.0+incompatible' is vulnerable",
            "upgrade @scope/pkg-name1234567890@1.2.3-rc.1, then retry",
            // The `+` of build metadata, in a word of its own long enough to matter.
            "Package 'github.com/o/r@v1.2.3+build20210101abcdef123456' ok",
            // `_` is a name character.
            "Package '@scope/my_pkg1234567890@1.2.3' ok",
            // A module path with no version.
            "Package 'github.com/xuri/excelize/v2' is vulnerable",
            "no required module provides package github.com/xuri/excelize/v2; run go get",
            // Every coordinate of a text reads whole, multibyte text beside them too.
            "github.com/xuri/excelize/v2@2.10.1 and @scope/pkg-name1234567890@1.2.3",
            "é github.com/xuri/excelize/v2@2.10.1 é @scope/pkg-name1234567890@1.2.3 é",
        ] {
            assert_eq!(mask_secrets(text), text);
        }
    }

    /// A coordinate exempts a word only as a package name: user-info on it is
    /// masked, a bare token is masked, and so is a token in a URL's or a module
    /// path's segment, which is where webhook and API secrets sit (the shapes the
    /// sprint review found past the first version of the exemption).
    #[test]
    fn a_secret_beside_a_package_coordinate_is_still_masked() {
        let token = "Q1w2E3r4T5y6U7i8O9p0A1s2D3f4G5h6";
        for (text, secret) in [
            ("https://user:Tok3n1234567890abcdefgh@registry.example/pkg@1.2.3", "Tok3n1234567890abcdefgh"),
            ("leaked a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0 here", "a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0"),
            // User-info with no scheme, and a bare token before a version.
            ("connect user:Tok3n1234567890abcdefgh@10.0.0.1 now", "Tok3n1234567890abcdefgh"),
            ("connect root:Tok3n1234567890abcdefgh@192.168.1.5:5432/db", "Tok3n1234567890abcdefgh"),
            ("sent Tok3n1234567890abcdefgh@1.2.3", "Tok3n1234567890abcdefgh"),
            ("sent a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0@v2.0.0 x", "a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0"),
            // A name that is one token-shaped segment is a token, not a name.
            ("sent pkg-name1234567890abcdefgh@1.2.3 x", "pkg-name1234567890abcdefgh"),
            // The `@` is not followed by a version: an e-mail address, not a coordinate.
            (&format!("sent {token}@example.com"), token),
            // One numeric component is not a version.
            (&format!("sent {token}@2 today"), token),
            // The token is not the whole name: `=` is a token character, not a name one.
            (&format!("key=x{token}@1.2.3"), token),
            // The token spills past the right edge of the coordinate.
            (&format!("github.com/o/r@1.2.3/{token}"), token),
            (&format!("pkg@1.2.3_{token}"), token),
            // No dot-qualified host: a path, not a module path.
            (&format!("sent {token}/xuri/excelize/v2 today"), token),
            (&format!("sent abcdefghij/{token} today"), token),
            // A secret in the path of a URL or of a dot-qualified host.
            ("POST https://hooks.slack.com/services/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 failed", "Xy7Kq9Lm2Np4Rs6Tv8Uw0Zab"),
            ("POST https://discord.com/api/webhooks/123456789012345678/Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0Uv1Wx2Yz3", "Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0Uv1Wx2Yz3"),
            (&format!("GET https://api.example.com/v1/{token}"), token),
            (&format!("see example.com/{token}"), token),
            (&format!("fetch x.io/{token}"), token),
            (&format!("git clone https://github.com/org/{token}.git"), token),
            ("host.com/ghp_16C7e42F292c6912E7710c838347Ae178B4a", "ghp_16C7e42F292c6912E7710c838347Ae178B4a"),
            ("registry.npmjs.org/-/npm/v1/tokens/npm_Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0Uv1Wx2Yz3", "npm_Ab1Cd2Ef3Gh4Ij5Kl6Mn7Op8Qr9St0Uv1Wx2Yz3"),
            ("s3.amazonaws.com/bucket/wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY", "wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY"),
            // After `://` a short segment is not read as a package path either.
            ("POST https://example.com/services/T0A1B2C3D/B0A1B2C3D/short16Chars9", "short16Chars9"),
            // Beside a coordinate, not in it.
            (&format!("{token} github.com/xuri/excelize/v2@2.10.1"), token),
        ] {
            let masked = mask_secrets(text);
            assert!(!masked.contains(secret), "{secret:?} survives in {masked:?}");
            assert!(masked.contains(MASK), "{masked:?}");
        }
        assert_eq!(
            mask_secrets(&format!("{token} github.com/xuri/excelize/v2@2.10.1")),
            format!("{MASK} github.com/xuri/excelize/v2@2.10.1"),
        );
        // A token between two coordinates: only it goes.
        assert_eq!(
            mask_secrets(&format!("github.com/xuri/excelize/v2@2.10.1 {token} @scope/pkg-name1234567890@1.2.3")),
            format!("github.com/xuri/excelize/v2@2.10.1 {MASK} @scope/pkg-name1234567890@1.2.3"),
        );
        // The user-info goes and the coordinate after it reads whole.
        assert_eq!(
            mask_secrets("https://Tok3n1234567890abcdefgh@registry.example/@scope/pkg-name1234567890@1.2.3"),
            format!("https://{MASK}@registry.example/@scope/pkg-name1234567890@1.2.3"),
        );
    }

    #[test]
    fn the_excerpt_keeps_the_tail_and_is_bounded() {
        let long: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let e = excerpt(&long);
        assert_eq!(e.lines().count(), EXCERPT_LINES);
        assert!(e.ends_with("line 39"));
        // One ASCII byte last, so `len - EXCERPT_BYTES` falls inside a two-byte `é`.
        let wide = format!("{}x", "é".repeat(2000));
        let e = excerpt(&wide);
        assert!(e.len() <= EXCERPT_BYTES + "…".len(), "{}", e.len());
        // A secret is masked before the cut, so no fragment of it survives: here
        // the byte window would start ten characters before the token's end,
        // leaving a fragment too short for the token rule.
        let token = "Q1w2E3r4T5y6U7i8O9p0A1s2D3f4G5h6";
        let straddle = format!("lead {token}{}", "z ".repeat((EXCERPT_BYTES - 10) / 2));
        assert!(!excerpt(&straddle).contains(&token[token.len() - 10..]), "{}", excerpt(&straddle));
    }
}
