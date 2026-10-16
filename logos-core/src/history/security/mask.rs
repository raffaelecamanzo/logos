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
//!   `=>`, `:=`, `==`, and the key and value bare, quoted or escape-quoted
//!   (`\"token\": \"…\"`). A quoted value is masked to its closing quote,
//!   spaces and all; an unquoted one to the next space, quote or comma, so a
//!   bare value's later words stay (the log keeps the whole text). The short key words (`pass`, `pw`, `pwd`,
//!   `auth`) count only as a whole segment of a key (`DB_PASS`, `x-auth`), so
//!   `passed` and `author` are not keys;
//! - the credential after a bare `Bearer` or `Basic`, key or no key;
//! - any word of 20 or more token characters that mixes letters and digits —
//!   the shape of a generated key or token, whatever its provider.
//!
//! [CR-217]: ../../../../docs/requests/CR-217-security-findings-in-graph-context.md
//! [FR-SC-03]: ../../../../docs/specs/requirements/FR-SC-03.md
//! [BR-67]: ../../../../docs/specs/software-spec.md#328-security-findings

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

/// A credential-named key, its separator and an optional auth scheme (group 1),
/// then the value: quoted to its closing quote, or unquoted (group 2).
static CREDENTIAL_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b(?:[a-z0-9_\-.]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|client[_-]?key|credential|authorization|cookie|session[_-]?id)[a-z0-9_\-]*|(?:[a-z0-9]+[_\-.])*(?:pass|pw|pwd|auth)(?:[_\-.][a-z0-9]+)*\b)\\?["']?\s*(?:=>|:=|==|[:=]|\s+is\s+|\s+)\s*(?:\\?["']?\s*(?:bearer|basic|token|digest)\s+)?)(\\"[^"\\\n]*(?:\\")?|"(?:[^"\\\n]|\\.)*"?|'[^'\n]*'?|[^\s"',\\]+)"#,
    )
    .expect("the credential pattern compiles")
});

static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(\b(?:bearer|basic)\s+)([^\s"',]+)"#).expect("the bearer pattern compiles")
});

static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_\-+/=]{20,}").expect("the token pattern compiles"));

/// `text` with every span the [module docs](self) list replaced by [`MASK`].
pub fn mask_secrets(text: &str) -> String {
    let text = PEM.replace_all(text, MASK);
    let text = URL_USERINFO.replace_all(&text, format!("${{1}}{MASK}@").as_str());
    let text = AWS_KEY_ID.replace_all(&text, MASK);
    let text = CREDENTIAL_VALUE.replace_all(&text, |caps: &regex::Captures<'_>| {
        // A quoted value keeps its quotes, so the surrounding text still reads;
        // an escaped-quoted one (`\"…\"`, JSON inside a JSON string) too.
        let value = &caps[2];
        let quote = if value.starts_with("\\\"") {
            Some("\\\"".to_string())
        } else {
            value.chars().next().filter(|c| *c == '"' || *c == '\'').map(String::from)
        };
        let (open, close) = quote.map_or((String::new(), String::new()), |q| (q.clone(), q));
        format!("{}{open}{MASK}{close}", &caps[1])
    });
    let text = BEARER.replace_all(&text, format!("${{1}}{MASK}").as_str());
    TOKEN
        .replace_all(&text, |caps: &regex::Captures<'_>| {
            let word = &caps[0];
            let mixed = word.bytes().any(|b| b.is_ascii_digit())
                && word.bytes().any(|b| b.is_ascii_alphabetic());
            if mixed {
                MASK.to_string()
            } else {
                word.to_string()
            }
        })
        .into_owned()
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
        ] {
            assert_eq!(mask_secrets(text), text);
        }
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
