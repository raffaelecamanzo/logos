//! The web contribution point: routes an extension mounts on the localhost web
//! surface (feature `web`).
//!
//! An extension contributes routes one at a time, each a path, a method and a
//! handler already bound to the extension's own state. The web adapter mounts
//! them beside its core route table, **inside** its guard stack — loopback host,
//! CSP, the method guard and the intent guard — and admits each contributed
//! `POST` route past the method guard: declaring a [`WebRoute::post`] is the
//! extension's entry in the `POST` allow-list ([NFR-SE-06]). The guard admits a
//! `POST` by the route the router itself matched, so a mounted `POST` can never
//! be one the guard refuses, nor an admitted one that nothing handles.
//!
//! A path that is both one of the extension's SPA views and the target of one of
//! its `POST`s (chat's `/chat`) also declares a [`WebRoute::shell`]: the `GET` a
//! browser navigation sends there is answered with the web surface's own SPA
//! shell, exactly as an unrouted view path is by its history fallback.
//!
//! [NFR-SE-06]: ../../docs/specs/requirements/NFR-SE-06.md

use std::fmt;
use std::sync::Arc;

use axum::handler::Handler;
use axum::routing::MethodRouter;
use logos_core::federation::Backing;
use logos_core::Engine;
use serde::Serialize;
use serde_json::Value;

/// What an extension's routes are built over: the serve's default engine and
/// its backing (the single engine, or the workspace member registry).
#[derive(Clone)]
pub struct WebContext {
    /// The engine the shared single-root surfaces run against.
    pub engine: Arc<Engine>,
    /// The serve backing.
    pub backing: Arc<Backing<Engine>>,
}

/// An HTTP method an extension route answers. The web surface is read-only
/// except its enumerated `POST`s, so these are the only two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WebMethod {
    /// A read.
    Get,
    /// A mutation, admitted by the method guard and gated by the intent guard.
    Post,
}

impl WebMethod {
    /// The method's name.
    pub const fn as_str(self) -> &'static str {
        match self {
            WebMethod::Get => "GET",
            WebMethod::Post => "POST",
        }
    }
}

/// One route an extension mounts.
#[derive(Clone)]
pub struct WebRoute {
    path: &'static str,
    method: WebMethod,
    /// The extension's handler, bound to its own state — or `None` for a
    /// [`WebRoute::shell`] route, which the web surface answers with its SPA shell.
    service: Option<MethodRouter>,
}

impl WebRoute {
    /// A `GET` route at `path` (axum syntax: `:param`, `*rest`), answered by
    /// `handler` over `state`.
    pub fn get<H, T, S>(path: &'static str, handler: H, state: S) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
        S: Clone + Send + Sync + 'static,
    {
        Self {
            path,
            method: WebMethod::Get,
            service: Some(axum::routing::get(handler).with_state(state)),
        }
    }

    /// A `POST` route at `path`, answered by `handler` over `state` — and this
    /// extension's entry in the web surface's `POST` allow-list.
    pub fn post<H, T, S>(path: &'static str, handler: H, state: S) -> Self
    where
        H: Handler<T, S>,
        T: 'static,
        S: Clone + Send + Sync + 'static,
    {
        Self {
            path,
            method: WebMethod::Post,
            service: Some(axum::routing::post(handler).with_state(state)),
        }
    }

    /// A `GET` at `path` the web surface answers with its SPA shell: a browser
    /// navigation to one of this extension's client views, at a path the
    /// extension also mounts a `POST` on. Without it that navigation would be
    /// refused, because a routed path's method router answers only the methods
    /// mounted on it, and the history fallback runs only for unrouted paths.
    pub fn shell(path: &'static str) -> Self {
        Self {
            path,
            method: WebMethod::Get,
            service: None,
        }
    }

    /// The route's path pattern.
    pub fn path(&self) -> &'static str {
        self.path
    }

    /// The route's method.
    pub fn method(&self) -> WebMethod {
        self.method
    }

    /// Is this a [`WebRoute::shell`] route, answered by the web surface's SPA
    /// shell rather than by a handler of the extension's?
    pub fn is_shell(&self) -> bool {
        self.service.is_none()
    }

    /// The bound handler, for the web adapter to mount — `None` for a
    /// [`WebRoute::shell`] route, which the web adapter answers itself.
    pub fn into_service(self) -> Option<MethodRouter> {
        self.service
    }
}

impl fmt::Debug for WebRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebRoute")
            .field("path", &self.path)
            .field("method", &self.method)
            .field("shell", &self.is_shell())
            .finish_non_exhaustive()
    }
}

/// A field an extension adds to the Dashboard's overview read-model
/// (`GET /api/v1/overview`, [FR-UI-09]): its key in that object and how to read
/// it from the requested member's engine.
///
/// The Dashboard is one core read-model, and an extension's card on it reads its
/// data from that one response rather than from a route of its own — so its field
/// rides the same request, the same member scope and the same failure rule as the
/// core's fields: the web surface reads every field inside one blocking bridge,
/// and a field that fails fails the response, as a required core read does. The
/// SPA half is the extension's dashboard card ([`crate::ui::UiDashboardCard`]).
///
/// [FR-UI-09]: ../../docs/specs/requirements/FR-UI-09.md
#[derive(Clone)]
pub struct OverviewField {
    key: &'static str,
    read: Arc<OverviewRead>,
}

/// How an [`OverviewField`] is read: the member's engine in, the field's JSON out.
pub type OverviewRead = dyn Fn(&Engine) -> anyhow::Result<Value> + Send + Sync;

impl OverviewField {
    /// The field `key`, read by `read`.
    ///
    /// `key` must not be one of the core's own overview keys; the web surface
    /// drops a field that would shadow one, as the MCP adapter drops a tool that
    /// would shadow a core tool.
    pub fn new<T: Serialize>(
        key: &'static str,
        read: impl Fn(&Engine) -> anyhow::Result<T> + Send + Sync + 'static,
    ) -> Self {
        Self {
            key,
            read: Arc::new(move |engine: &Engine| Ok(serde_json::to_value(read(engine)?)?)),
        }
    }

    /// The field's key in the overview object.
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// Read the field over `engine`.
    ///
    /// # Errors
    /// Whatever the reader reports, or a value that does not serialise.
    pub fn read(&self, engine: &Engine) -> anyhow::Result<Value> {
        (self.read)(engine)
    }
}

impl fmt::Debug for OverviewField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverviewField").field("key", &self.key).finish_non_exhaustive()
    }
}
