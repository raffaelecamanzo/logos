//! `xservice_declared_topics`, the twin of `logos xservice declared-topics`
//! ([FR-WS-36]): a dynamic route ([`ToolRoute::new_dyn`]), not an `#[tool]`
//! method, because `LogosMcp` already carries more tool methods than the
//! `no_god_containers` threshold allows. Registered on the federated backing
//! only, answered through the `run_xservice` bridge, and holding its own
//! stamp-cached [`DeclaredTopics`].
//!
//! [FR-WS-36]: ../../../docs/specs/requirements/FR-WS-36.md

use std::sync::Arc;

use logos_core::federation::{xservice_declared_topics, DeclaredTopics};
use rmcp::handler::server::common::schema_for_type;
use rmcp::handler::server::tool::{ToolCallContext, ToolRoute};
use rmcp::model::{ErrorData, Tool};

use crate::server::{LogosMcp, XserviceRepoParams};

/// The tool's registered name.
const NAME: &str = "xservice_declared_topics";

/// What an agent reads: a declaration, never an observed broker edge (BR-54).
const DESCRIPTION: &str = "Declared topics (FR-WS-36) — a declaration, not an observed broker edge: per captured topic (`topics`, each with its `topic` identity), every member whose committed `src/main` configuration declares it (`declarations`), each naming the member, the canonical configuration `key`, the `file` (member-relative), the `profile` (null for the unprofiled base) and `status`: `observed` (the member already shows on the topic through an observed producer or consumer) or `net-new` (configuration adds the pair; its `site` says why the member shows no observed edge: `config-only`, `site-refused`, `site-uncaptured` or `site-unread`). THIS IS A DECLARATION, NOT AN OBSERVED BROKER EDGE (BR-54): no row is a bridge edge, a resolved call, a producer or a consumer or a coverage figure, and none may be read as one — read `xservice_route_providers` for what is observed. `declared_only` lists values no member's broker site names — not a topic, never drawn as one — and `refused` the topic-named values the placeholder hop refused, each with its reason. `headline` is `workspace_status`'s `declared_topics` section, workspace-wide whatever the scope: `declared_topic_pairs` (split `already_observed` and `net_new`, the net-new pairs split by site) beside its denominator `declarations_considered` and the `members` read — a member whose engine or facts could not be read is named in `members.unread` with `members.unread_reasons` inside the answer, never an error and never counted as a member declaring nothing. `repo` scopes `topics`, `declared_only` and `refused` to one declaring member; a name that is not a member read yields an empty list with `scope_note` stating why.";

/// The route, with its own relation holder.
pub(crate) fn route() -> ToolRoute<LogosMcp> {
    let holder = Arc::new(DeclaredTopics::new());
    let attr = Tool::new(NAME, DESCRIPTION, schema_for_type::<XserviceRepoParams>());
    ToolRoute::new_dyn(attr, move |context: ToolCallContext<'_, LogosMcp>| {
        let holder = Arc::clone(&holder);
        let arguments = context.arguments.unwrap_or_default();
        let service = context.service;
        Box::pin(async move {
            let params: XserviceRepoParams = serde_json::from_value(serde_json::Value::Object(arguments))
                .map_err(|err| ErrorData::invalid_params(err.to_string(), None))?;
            service
                .run_xservice(NAME, move |reg, _bridge| {
                    xservice_declared_topics(&holder.relation(reg), params.repo.as_deref())
                })
                .await
        })
    })
}
