//! The two-member workspace the S-480 / S-484 agent-tool suites run over: real
//! indexed stores (`api`, `web`), one bound cross-service edge, one build
//! dependency, `web`'s own config, a loose file of the workspace root and a docs
//! directory beside the members.
//!
//! Shared by path, not copied: `agent-core/tests/addressed_workspace_tools.rs`
//! (the tools' own behaviour) and `web/tests/mcp_parity_workspace_tools.rs` (the
//! tools against their MCP twins) must read the same workspace, or a payload
//! that differs between them could be a fixture that drifted rather than a tool
//! that diverged. Everything here is fixture plumbing: no assertions live in this
//! file.

#![allow(dead_code)] // each including binary uses a different subset

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::XserviceBacking;
use logos_core::federation::{
    Backing, ContractBridge, EngineRegistry, Federation, Governance, Member, RegistryMode,
    ServiceBoundary, ServiceLayer,
};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

const API_LIB: &str = "pub fn shared() {}\npub fn api_only() { shared(); }\n";
const WEB_LIB: &str = "pub fn shared() {}\npub fn render() { shared(); }\n";

/// A literal client call in `web` and the axum route in `api` it binds — one
/// resolved cross-service edge, so reachability and the boundary rule have a
/// binding to read (the shape `chat-agent/tests/xservice_roster.rs` uses).
const WEB_BOUND_CLIENT: &str = r#"
use reqwest::Client;

pub async fn fetch_user(client: Client) {
    let _ = client.get("/users/{id}").await;
}
"#;
const API_ROUTE: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

/// `web` builds against `api`'s artifact — one build-dependency pair, so the
/// `xservice_build_deps` payload compared below is not an empty one.
const API_POM: &str = "<project><groupId>com.shop</groupId><artifactId>api</artifactId>\
    <version>1</version></project>";
const WEB_POM: &str = "<project><groupId>com.shop</groupId><artifactId>web</artifactId>\
    <version>1</version><dependencies><dependency><groupId>com.shop</groupId>\
    <artifactId>api</artifactId></dependency></dependencies></project>";

/// `web`'s own config: an ignored directory and a read root `api` does not have,
/// so each sandbox is provably the member's own.
const WEB_CONFIG: &str = "[semantics]\nignored_dirs = [\"generated\"]\n\n\
    [chat]\nread_roots = [\"../shared-docs\"]\n";

pub fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

fn index(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink");
}

/// The two-member workspace on disk: real indexed stores, one bound edge, a
/// build dependency, `web`'s own config, a loose file of the workspace root and
/// a docs directory beside the members.
pub struct Workspace {
    tmp: TempDir,
}

impl Workspace {
    pub fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        let (api, web) = (root.join("api"), root.join("web"));
        write(&api, "src/lib.rs", API_LIB);
        write(&api, "src/main.rs", API_ROUTE);
        write(&api, "pom.xml", API_POM);
        write(&api, "generated/stub.rs", "// api's generated code\n");
        write(&web, "src/lib.rs", WEB_LIB);
        write(&web, "src/client.rs", WEB_BOUND_CLIENT);
        write(&web, "pom.xml", WEB_POM);
        write(&web, "generated/stub.rs", "// web's generated code\n");
        write(&web, ".logos/config.toml", WEB_CONFIG);
        write(root, "NOTES.md", "the workspace root's own file\n");
        write(
            root,
            "shared-docs/guide.md",
            "a guide both members link to\n",
        );
        #[cfg(unix)]
        {
            // `web` reaches a sibling member, the workspace root's own file and
            // the shared docs through in-tree links; `api` links the docs too.
            symlink("../api", &web.join("linked-api"));
            symlink("../NOTES.md", &web.join("root-notes.md"));
            symlink("../shared-docs", &web.join("docs"));
            symlink("../shared-docs", &api.join("docs"));
        }
        index(&api);
        index(&web);
        Self { tmp }
    }

    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// The federation over the two members; `governance` declares `web` (the
    /// frontend) may not call `api` (the backend), which the bound edge breaks.
    pub fn federation(&self, governance: bool) -> Federation {
        let root = self.root();
        let governance = if governance {
            Governance {
                service_layers: vec![
                    ServiceLayer {
                        name: "frontend".into(),
                        members: vec!["web".into()],
                    },
                    ServiceLayer {
                        name: "backend".into(),
                        members: vec!["api".into()],
                    },
                ],
                boundaries: vec![ServiceBoundary {
                    from: "frontend".into(),
                    to: "backend".into(),
                    reason: Some("the frontend goes through the gateway".into()),
                }],
                no_cross_service_callers: Vec::new(),
            }
        } else {
            Governance::default()
        };
        Federation {
            name: "shop".to_string(),
            root: root.to_path_buf(),
            members: vec![
                Member {
                    name: "api".to_string(),
                    root: root.join("api"),
                },
                Member {
                    name: "web".to_string(),
                    root: root.join("web"),
                },
            ],
            default: None,
            links: Vec::new(),
            governance,
            warm_concurrency: None,
            member_kinds: Default::default(),
        }
    }

    pub fn registry(&self, governance: bool) -> EngineRegistry<Engine> {
        EngineRegistry::<Engine>::new(self.federation(governance), RegistryMode::Lazy)
    }

    pub fn backing(&self, governance: bool) -> XserviceBacking {
        let backing = Arc::new(Backing::Federated(Box::new(self.registry(governance))));
        XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
            .expect("a federated backing mints a workspace backing")
    }
}

pub fn args(value: Value) -> String {
    value.to_string()
}
