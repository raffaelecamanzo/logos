//! The SPA bundle is an injected asset provider (S-641, [FR-UI-51], CR-213).
//!
//! The router serves the SPA from a [`SpaAssets`] provider it is composed with. The
//! default is the bundle embedded in the binary, and it must serve **exactly** the
//! bytes that bundle holds — asserted here against the embedded files themselves,
//! not by eye. A different provider (a binary embedding its own
//! bundle) is served through the very same shell, asset and
//! history-fallback paths, with the same consistency guard.
//!
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use logos_core::federation::Backing;
use logos_core::Engine;
use logos_ext::ExtensionHost;
use tempfile::TempDir;
use tower::ServiceExt;
use web::spa::{content_type, EmbeddedSpa, Spa, SpaAssets};
use web::IntentToken;

const HOST: &str = "127.0.0.1:4983";

/// A router over a fresh project, serving `assets` (or the default bundle), and
/// the session's intent token.
fn router(assets: Option<Arc<dyn SpaAssets>>) -> (TempDir, Router, String) {
    let dir = TempDir::new().expect("temp dir");
    let engine = Arc::new(Engine::start(dir.path()).expect("engine starts"));
    let backing = Arc::new(Backing::Single(engine));
    let intent = IntentToken::generate();
    let token = intent.as_str().to_string();
    let host = ExtensionHost::new();
    let router = match assets {
        Some(assets) => web::hosted_router_with_assets(backing, intent, &host, assets),
        None => web::hosted_router(backing, intent, &host),
    }
    .expect("the router builds");
    (dir, router, token)
}

async fn get(router: &Router, path: &str, accept: Option<&str>) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut request = Request::get(path).header(header::HOST, HOST);
    if let Some(accept) = accept {
        request = request.header(header::ACCEPT, accept);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().expect("ASCII").to_string());
    let body = response.into_body().collect().await.expect("a body").to_bytes().to_vec();
    (status, mime, body)
}

/// The embedded files, as `(dist-relative path, bytes)`.
///
/// Reports its denominator, and refuses to be vacuous: a tree with no Vite build
/// embeds only the committed placeholder shell, so every "each asset byte for
/// byte" loop below then checks the shell alone — stated on stderr, never read as
/// "all assets identical". If a real build IS on disk (`ui/dist/assets/`) yet the
/// embed holds no asset, the embed is stale and the tests fail rather than pass on
/// nothing.
fn embedded() -> Vec<(String, Vec<u8>)> {
    let files: Vec<(String, Vec<u8>)> = Spa::iter()
        .map(|path| {
            let data = Spa::get(&path).expect("an iterated file resolves").data.into_owned();
            (path.to_string(), data)
        })
        .collect();
    let assets = files.iter().filter(|(path, _)| path != "index.html").count();
    eprintln!("embedded bundle: {} file(s), {assets} besides the shell", files.len());
    let built = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/dist/assets").is_dir();
    assert!(
        !built || assets > 0,
        "web/ui/dist/assets exists but the binary embeds no asset: a stale embed \
         (`touch web/src/spa.rs` and rebuild)"
    );
    files
}

/// An in-memory provider — what another binary's own embed looks like to the
/// router.
struct MapAssets(BTreeMap<&'static str, &'static [u8]>);

impl SpaAssets for MapAssets {
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.0.get(path).map(|bytes| Cow::Borrowed(*bytes))
    }
}

const PRIVATE_SHELL: &str =
    "<!doctype html><html><head><title>private</title></head><body>\
     <script type=\"module\" src=\"/assets/private-1.js\"></script></body></html>";

fn private_bundle() -> Arc<dyn SpaAssets> {
    Arc::new(MapAssets(BTreeMap::from([
        ("index.html", PRIVATE_SHELL.as_bytes()),
        ("assets/private-1.js", b"export const private = 1;".as_slice()),
        ("private.css", b"body{}".as_slice()),
    ])))
}

/// The default provider hands out the embedded bundle's bytes, file for file.
#[test]
fn the_default_provider_serves_the_embedded_bundle_byte_for_byte() {
    let files = embedded();
    assert!(!files.is_empty(), "a bundle is embedded (at least the placeholder shell)");
    for (path, bytes) in &files {
        let served = EmbeddedSpa.get(path).unwrap_or_else(|| panic!("{path} is served"));
        assert_eq!(served.as_ref(), bytes.as_slice(), "{path}");
    }
    assert!(EmbeddedSpa.get("assets/does-not-exist-xyz.js").is_none());
}

/// Through the default router every embedded file but the shell is served
/// byte-identical, with its content type; the shell is the same bytes plus the
/// intent token's `<meta>` tag and nothing else.
#[tokio::test]
async fn the_default_router_serves_every_embedded_asset_byte_for_byte() {
    let (_dir, router, token) = router(None);
    let files = embedded();
    for (path, bytes) in files.iter().filter(|(path, _)| path != "index.html") {
        let (status, mime, body) = get(&router, &format!("/{path}"), Some("*/*")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(mime.as_deref(), Some(content_type(path)), "{path}");
        assert_eq!(&body, bytes, "{path} is served byte-for-byte");
    }

    let shell = files.iter().find(|(path, _)| path == "index.html").expect("a shell").1.clone();
    let shell = String::from_utf8(shell).expect("the shell is UTF-8");
    let tag = format!("<meta name=\"logos-intent\" content=\"{token}\">");
    let expected = match shell.find("</head>") {
        Some(at) => format!("{}{tag}{}", &shell[..at], &shell[at..]),
        None => format!("{tag}{shell}"),
    };
    let (status, _, body) = get(&router, "/", Some("text/html")).await;
    // A consistent bundle serves its shell; a build whose shell names assets it
    // does not embed serves the diagnostic instead (the CR-049 guard), unchanged.
    if status == StatusCode::OK {
        assert_eq!(String::from_utf8(body).expect("UTF-8"), expected);
    } else {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}

/// The history fallback and the 404s, for the default provider: an HTML
/// navigation to a client route is the shell, any other miss is an honest 404.
#[tokio::test]
async fn the_history_fallback_behaves_as_before_for_the_default_provider() {
    let (_dir, router, _) = router(None);
    let (root_status, _, root) = get(&router, "/", Some("text/html")).await;
    for client_route in ["/dashboard", "/graph/deep/route", "/config"] {
        let (status, _, body) = get(&router, client_route, Some("text/html,application/xhtml+xml")).await;
        assert_eq!((status, &body), (root_status, &root), "{client_route} resolves to the shell");
        let (status, _, _) = get(&router, client_route, Some("application/json")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{client_route} without an HTML Accept");
    }
    let (status, _, _) = get(&router, "/assets/does-not-exist-xyz.js", Some("*/*")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // `index.html` is the shell, never a raw asset: the token is always injected.
    let (status, _, _) = get(&router, "/index.html", Some("*/*")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// An injected provider is what the router serves: its shell at `/` and on the
/// history fallback (with the intent token), its assets by path with the right
/// content type — and nothing of the embedded bundle.
#[tokio::test]
async fn an_injected_provider_is_served_through_the_same_shell_asset_and_fallback_paths() {
    let (_dir, router, token) = router(Some(private_bundle()));

    let tag = format!("<meta name=\"logos-intent\" content=\"{token}\">");
    let expected = PRIVATE_SHELL.replace("</head>", &format!("{tag}</head>"));
    for path in ["/", "/some/client/route"] {
        let (status, _, body) = get(&router, path, Some("text/html")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(String::from_utf8(body).expect("UTF-8"), expected, "{path}");
    }

    let (status, mime, body) = get(&router, "/assets/private-1.js", Some("*/*")).await;
    assert_eq!((status, mime.as_deref()), (StatusCode::OK, Some("text/javascript; charset=utf-8")));
    assert_eq!(body, b"export const private = 1;");
    let (status, mime, body) = get(&router, "/private.css", Some("*/*")).await;
    assert_eq!((status, mime.as_deref()), (StatusCode::OK, Some("text/css; charset=utf-8")));
    assert_eq!(body, b"body{}");

    // The embedded bundle's files are not in this provider, so they are not served.
    for (path, _) in embedded().iter().filter(|(path, _)| path != "index.html") {
        let (status, ..) = get(&router, &format!("/{path}"), Some("*/*")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} belongs to the embedded bundle");
    }
}

/// The consistency guard reads the injected provider: a shell naming an asset the
/// provider lacks is the loud diagnostic, not a blank page.
#[tokio::test]
async fn the_consistency_guard_reads_the_injected_provider() {
    let inconsistent: Arc<dyn SpaAssets> = Arc::new(MapAssets(BTreeMap::from([(
        "index.html",
        PRIVATE_SHELL.as_bytes(),
    )])));
    let (_dir, router, _) = router(Some(inconsistent));
    let (status, _, body) = get(&router, "/", Some("text/html")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let page = String::from_utf8(body).expect("UTF-8");
    assert!(page.contains("/assets/private-1.js"), "names the missing asset: {page}");
}

/// The request's path is never handed to a provider as a traversal: `/../x` and
/// friends are an honest 404 and the provider is not asked, so a provider backed
/// by a directory cannot be walked out of its root.
#[tokio::test]
async fn a_traversal_path_is_a_404_and_never_reaches_the_provider() {
    use std::sync::Mutex;
    /// Records the keys it is asked for; serves `assets/ok.js` and nothing else.
    struct Recording(Mutex<Vec<String>>);
    impl SpaAssets for Recording {
        fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
            self.0.lock().expect("lock").push(path.to_string());
            match path {
                "assets/ok.js" => Some(Cow::Borrowed(b"ok".as_slice())),
                _ => None,
            }
        }
    }
    let recording = Arc::new(Recording(Mutex::new(Vec::new())));
    let (_dir, router, _) = router(Some(recording.clone()));

    let (status, ..) = get(&router, "/assets/ok.js", Some("*/*")).await;
    assert_eq!(status, StatusCode::OK, "a plain key is served");
    let asked = recording.0.lock().expect("lock").len();

    for hostile in ["/../secret.txt", "/assets/../../etc/passwd", "/assets/./ok.js", "/assets//ok.js"] {
        let (status, ..) = get(&router, hostile, Some("*/*")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{hostile}");
    }
    assert_eq!(recording.0.lock().expect("lock").len(), asked, "the provider was never asked");
}

/// A provider that holds no shell cannot serve the SPA: `/` and a client route
/// answer an honest `500` (never a blank page, never another bundle's shell), and
/// `index.html` is not served as a raw asset.
#[tokio::test]
async fn a_provider_without_a_shell_answers_500_not_a_blank_page() {
    let shellless: Arc<dyn SpaAssets> = Arc::new(MapAssets(BTreeMap::from([(
        "assets/x.js",
        b"x".as_slice(),
    )])));
    let (_dir, router, token) = router(Some(shellless));
    for path in ["/", "/some/client/route"] {
        let (status, _, body) = get(&router, path, Some("text/html")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        assert!(!String::from_utf8_lossy(&body).contains(&token), "{path}: the token is not echoed");
    }
    let (status, ..) = get(&router, "/index.html", Some("*/*")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, ..) = get(&router, "/assets/x.js", Some("*/*")).await;
    assert_eq!(status, StatusCode::OK, "its assets are still served");
}
