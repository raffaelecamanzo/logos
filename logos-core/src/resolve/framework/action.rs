//! Handlers written `controller#action`, and the module blocks that qualify
//! them (S-567, [CR-179], [FR-FW-03], [NFR-RA-05]).
//!
//! # A `controller#action` string names a class's instance method
//!
//! A Rails route names its handler as a string — `get "/rss" => "home#index"`,
//! `get "/x", to: "stories#show"` — which no scope declares, so as written text
//! it binds nothing. A dialect captures such a string `@fw.route.handler.action`
//! and states, as a pattern setting, the suffix its framework appends to a
//! controller's name (`(#set! fw.route.handler.suffix "Controller")`): the
//! convention is query data, never a literal of this interpreter.
//! [`Facts::name_handler`] reads `a/b#c` as the class `A::BController` and its
//! method `c` — each `/`-separated segment camel-cased the way the framework
//! spells a constant from a file name (`story_urls` → `StoryUrls`), the last one
//! suffixed. Text of any other shape (no `#`, an empty side, a character no
//! constant or method name holds) names nothing, and the route keeps its node
//! with no edge.
//!
//! # A module block qualifies the controllers inside it
//!
//! `namespace :mod do … end` puts its controllers in `Mod`, and `scope module:
//! "v1" do … end` in `V1`. A dialect captures the block
//! (`@fw.route.module.scope`) with the module it names (`@fw.route.module`, a
//! symbol or a string), and every module block around a registration applies,
//! outermost first. A block whose module the file does not state
//! (`@fw.route.module.opaque`) leaves every handler inside naming nothing: a
//! handler read in the wrong module would bind another controller's action. A
//! controller written with a leading `/` (`"/admin/users#index"`) is absolute
//! and takes no module.
//!
//! # It binds through the constant index
//!
//! A route whose handler was read here binds with
//! [`binder::bind_constant_action`](crate::resolve::binder::bind_constant_action)
//! — the constant read from the top level, then the one instance method of the
//! action's name its declarations hold — not as a call, which on a constant
//! reaches the class's singleton methods alone (S-565).
//!
//! [CR-179]: ../../../../docs/requests/CR-179-ruby-constants-requires-ancestry-and-rails-routes-resolve.md
//! [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::{text, RouteMatch};

/// The pattern setting that states the suffix a framework appends to a
/// controller's name, read with
/// [`pattern_setting`](crate::plugin::queries::pattern_setting).
pub(super) const SUFFIX_SETTING: &str = "fw.route.handler.suffix";

/// A block that puts the controllers inside it in a module, by byte range.
#[derive(Debug, PartialEq, Eq)]
struct Module {
    start: usize,
    end: usize,
    /// The module's segments, camel-cased; `None` when the file does not state
    /// it.
    path: Option<Vec<String>>,
}

/// What a query declared about `controller#action` handlers and module blocks.
/// Held apart from [`RouteMatch`], like [`super::nest::Facts`], so a dialect
/// that declares none of it leaves every other dialect's scan untouched.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Facts {
    /// The written handler of each registration naming one this way, and the
    /// suffix its pattern set, by the registration's `at`.
    written: HashMap<usize, (String, String)>,
    modules: Vec<Module>,
    /// The `at` of every registration whose handler [`Facts::name_handler`]
    /// read as a controller's action.
    named: HashSet<usize>,
}

/// One query match's handler and module captures, read off during
/// [`generic_match`](super::generic_match)'s walk of its captures.
#[derive(Default)]
pub(super) struct Captures<'t> {
    handler: Option<Node<'t>>,
    module: Option<Node<'t>>,
    scope: Option<Node<'t>>,
    opaque: bool,
}

impl<'t> Captures<'t> {
    /// Read the capture `name` on `node` when it is one of this module's:
    /// `true` when it was. The `@fw.route.module.*` captures not named here
    /// exist only for the query's predicates.
    pub(super) fn read(&mut self, name: &str, node: Node<'t>) -> bool {
        match name {
            "fw.route.handler.action" => drop(self.handler.get_or_insert(node)),
            "fw.route.module" => drop(self.module.get_or_insert(node)),
            "fw.route.module.scope" => drop(self.scope.get_or_insert(node)),
            "fw.route.module.opaque" => self.opaque = true,
            other => return other.starts_with("fw.route.module."),
        }
        true
    }

    /// Record the match's facts: its written handler for the registration at
    /// `at`, with the `suffix` its pattern set, and the module block it
    /// declares.
    pub(super) fn record(&self, facts: &mut Facts, src: &[u8], at: usize, suffix: Option<&str>) {
        if let Some(handler) = self.handler {
            let written = crate::extract::refs::unquote(text(handler, src)).trim().to_string();
            facts.written.insert(at, (written, suffix.unwrap_or_default().to_string()));
        }
        if let Some(scope) = self.scope {
            let path = match self.module {
                Some(module) if !self.opaque => constant_path(symbol_name(text(module, src))),
                _ => None,
            };
            facts.modules.push(Module {
                start: scope.start_byte(),
                end: scope.end_byte(),
                path,
            });
        }
    }
}

impl Facts {
    /// Rewrite the handler of a registration that wrote one `controller#action`
    /// into `Module::ControllerSuffix::action`, when it reads as one inside
    /// module blocks the file states.
    pub(super) fn name_handler(&mut self, route: &mut RouteMatch) {
        let Some((written, suffix)) = self.written.get(&route.at) else {
            return;
        };
        let Some((controller, action)) = written.split_once('#') else {
            return;
        };
        let (absolute, controller) = match controller.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, controller),
        };
        let (Some(mut class), true) = (constant_path(controller), is_method_name(action)) else {
            return;
        };
        if let Some(last) = class.last_mut() {
            last.push_str(suffix);
        }
        let mut path: Vec<String> = Vec::new();
        if !absolute {
            // Blocks nest rather than partially overlap: in start order the
            // enclosing ones come first.
            let mut around: Vec<&Module> = self
                .modules
                .iter()
                .filter(|m| m.start <= route.at && route.at < m.end)
                .collect();
            around.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
            for module in around {
                match &module.path {
                    Some(segments) => path.extend(segments.iter().cloned()),
                    None => return,
                }
            }
        }
        path.extend(class);
        path.push(action.to_string());
        route.handler = Some(path.join("::"));
        self.named.insert(route.at);
    }

    /// `true` when the registration at `at` names its handler as a
    /// controller's action.
    pub(super) fn is_named(&self, at: usize) -> bool {
        self.named.contains(&at)
    }
}

/// A symbol's name — `:mod` and `:"mod"` are `mod` — or a string's text.
pub(super) fn symbol_name(written: &str) -> &str {
    let written = written.trim();
    crate::extract::refs::unquote(written.strip_prefix(':').unwrap_or(written))
}

/// The constant a `/`-separated name spells, each segment camel-cased
/// (`mod/story_urls` → `Mod::StoryUrls`); `None` when a segment is empty or
/// holds a character no constant does.
fn constant_path(name: &str) -> Option<Vec<String>> {
    name.split('/')
        .map(|segment| {
            let mut chars = segment.chars();
            let opens = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            (opens && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| camelize(segment))
        })
        .collect::<Option<Vec<String>>>()
        .filter(|segments| segments.iter().all(|s| !s.is_empty()))
}

/// `story_urls` → `StoryUrls`: each `_`-separated part with its first letter
/// upper-cased, the underscores dropped.
fn camelize(segment: &str) -> String {
    segment
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// `true` when `name` is a method name: an identifier, optionally closed by
/// `?` or `!`.
fn is_method_name(name: &str) -> bool {
    let body = name.strip_suffix(['?', '!']).unwrap_or(name);
    let mut chars = body.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_controller_name_is_camel_cased_per_segment() {
        let path = |name: &str| constant_path(name).map(|p| p.join("::"));
        assert_eq!(path("home").as_deref(), Some("Home"));
        assert_eq!(path("story_urls").as_deref(), Some("StoryUrls"));
        assert_eq!(path("mod/domains_ban").as_deref(), Some("Mod::DomainsBan"));
        assert_eq!(path("v1").as_deref(), Some("V1"));
        // Near misses: an empty segment, a separator other than `/`, a
        // character no constant holds, a leading digit, a lone underscore.
        for refused in ["", "a//b", "a/", "a::b", "a-b", "a b", "1st", "_"] {
            assert_eq!(path(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn a_method_name_may_close_on_one_predicate_or_bang() {
        for name in ["index", "twofa_verify", "destroy!", "valid?", "_private"] {
            assert!(is_method_name(name), "{name:?}");
        }
        for name in ["", "?", "in dex", "a?b", "a??", "1a", "a#b", "a.b"] {
            assert!(!is_method_name(name), "{name:?}");
        }
    }

    #[test]
    fn a_symbol_is_read_without_its_colon_and_a_string_without_its_quotes() {
        assert_eq!(symbol_name(":mod"), "mod");
        assert_eq!(symbol_name(":\"mod\""), "mod");
        assert_eq!(symbol_name("\"mod\""), "mod");
        assert_eq!(symbol_name("'mod'"), "mod");
        assert_eq!(symbol_name("mod"), "mod");
    }
}
