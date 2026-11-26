//! Django: URLconfs (with Django REST framework routers), class-based
//! views, management commands. Routes are followed from the root URLconf
//! (`ROOT_URLCONF`) through `include()`, so labels carry the full route;
//! URLconfs nothing includes answer with their own patterns.

use super::frameworks::django as table;
use super::summary::{Binding, Ex};
use super::{Build, Val};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(super) fn run(build: &mut Build) {
    let mut urls = Urls::default();
    // DRF routers and what is registered on them.
    for (path, summary) in build.all() {
        for (scope_index, scope) in summary.scopes.iter().enumerate() {
            for (name, binding) in &scope.names {
                if let Binding::Value(Ex::Call { func, .. }) = binding {
                    let constructor = build.external(&path, scope_index, func);
                    if constructor.is_some_and(|c| table::ROUTERS.contains(&c.as_str())) {
                        urls.routers
                            .push(Val::Bound(path.clone(), scope_index, name.clone()));
                    }
                }
            }
        }
    }
    for (path, summary) in build.all() {
        for (scope, call) in &summary.statement_calls {
            let Some((object, method)) = super::split_method(call.callee()) else {
                continue;
            };
            if method != table::REGISTER {
                continue;
            }
            let router = build.resolve(&path, *scope, &object, 0);
            if urls.routers.contains(&router) {
                urls.registrations
                    .push((router, path.clone(), *scope, call.clone()));
            }
        }
    }

    // From the root URLconf down, then every URLconf nothing included.
    let mut roots = Vec::new();
    for (path, summary) in build.all() {
        if let Some(Binding::Value(ex)) = summary.scopes[0].names.get(table::ROOT_URLCONF) {
            if let Some(module) = build.eval_str(&path, 0, ex, 0) {
                if let Some(found) = build.module_path(&path, &module) {
                    roots.push(found);
                }
            }
        }
    }
    for root in roots {
        urls.urlconf(build, &root, "");
    }
    for (path, summary) in build.all() {
        if summary.scopes[0].names.contains_key(table::URLPATTERNS) {
            urls.urlconf(build, &path, "");
        }
    }
    for router in urls.routers.clone() {
        if !urls.reached_routers.contains(&router) {
            urls.router(build, &router, "");
        }
    }
    urls.unrouted(build);
    management_commands(build);
}

#[derive(Default)]
struct Urls {
    routers: Vec<Val>,
    registrations: Vec<(Val, PathBuf, usize, Ex)>,
    reached_modules: HashSet<PathBuf>,
    reached_routers: HashSet<Val>,
    /// View classes and function views given a route.
    routed: HashSet<(PathBuf, usize)>,
}

impl Urls {
    fn urlconf(&mut self, build: &mut Build, module: &Path, prefix: &str) {
        if !self.reached_modules.insert(module.to_path_buf()) {
            return;
        }
        let Some(summary) = build.summary(module) else {
            return;
        };
        if let Some(Binding::Value(patterns)) = summary.scopes[0].names.get(table::URLPATTERNS) {
            self.patterns(build, module, 0, patterns, prefix, 0);
        }
    }

    fn patterns(
        &mut self,
        build: &mut Build,
        path: &Path,
        scope: usize,
        ex: &Ex,
        prefix: &str,
        depth: u32,
    ) {
        if depth > super::DEPTH {
            return;
        }
        match ex {
            Ex::List(items) => {
                for item in items {
                    self.patterns(build, path, scope, item, prefix, depth + 1);
                }
            }
            Ex::Add(a, b) => {
                self.patterns(build, path, scope, a, prefix, depth + 1);
                self.patterns(build, path, scope, b, prefix, depth + 1);
            }
            Ex::Ref(parts) => {
                // `router.urls`
                if parts.len() >= 2 && parts[parts.len() - 1] == table::ROUTER_URLS {
                    let object = Ex::Ref(parts[..parts.len() - 1].to_vec());
                    let router = build.resolve(path, scope, &object, 0);
                    if self.routers.contains(&router) {
                        self.router(build, &router, prefix);
                        return;
                    }
                }
                if let Val::Bound(target, target_scope, name) = build.resolve(path, scope, ex, 0) {
                    if let Some(value) = build.bound_ex(&target, target_scope, &name) {
                        self.patterns(build, &target, target_scope, &value, prefix, depth + 1);
                    }
                }
            }
            Ex::Call { func, .. } => {
                let Some(name) = build.external(path, scope, func) else {
                    return;
                };
                let regex = table::REGEX_PATH.contains(&name.as_str());
                if !regex && !table::PATH.contains(&name.as_str()) {
                    return;
                }
                let route = match ex.param(0, "route") {
                    Some(route) => build.piece(path, scope, route).render(),
                    None => return,
                };
                let route = if regex {
                    route
                        .strip_prefix('^')
                        .unwrap_or(&route)
                        .trim_end_matches('$')
                        .to_string()
                } else {
                    route
                };
                let full = format!("{prefix}{route}");
                let Some(view) = ex.param(1, "view") else {
                    return;
                };
                if let Ex::Call { func, args, .. } = view {
                    let include = build.external(path, scope, func);
                    if include.is_some_and(|n| table::INCLUDE.contains(&n.as_str())) {
                        match args.first() {
                            Some(Ex::Str(module)) => {
                                if let Some(target) = build.module_path(path, module) {
                                    self.urlconf(build, &target, &full);
                                }
                            }
                            // `include((patterns, "app"))`
                            Some(Ex::List(items)) => {
                                if let Some(first) = items.first() {
                                    self.patterns(build, path, scope, first, &full, depth + 1);
                                }
                            }
                            Some(other) => {
                                self.patterns(build, path, scope, other, &full, depth + 1)
                            }
                            None => {}
                        }
                        return;
                    }
                }
                self.view(build, path, scope, view, &full);
            }
            _ => {}
        }
    }

    /// A route's view: a function, `View.as_view()`, or a viewset's
    /// `as_view({"get": "list"})`.
    fn view(&mut self, build: &mut Build, path: &Path, scope: usize, view: &Ex, route: &str) {
        let route = display(route);
        if let Some((class_path, class)) = build.as_view_class(path, scope, view) {
            self.routed.insert((class_path.clone(), class));
            if let Some(Ex::Dict(mapping)) = view.arg(0) {
                let methods = build.methods(&class_path, class);
                for (method, action) in mapping {
                    let (Ex::Str(method), Ex::Str(action)) = (method, action) else {
                        continue;
                    };
                    if let Some((_, def_path, def)) = methods.iter().find(|(n, ..)| n == action) {
                        let label = format!("{} {route}", method.to_uppercase());
                        build.emit(def_path, *def, "http", label);
                    }
                }
                return;
            }
            for (method, def_path, def) in build.view_handlers(&class_path, class) {
                let label = match method {
                    Some(method) => format!("{method} {route}"),
                    None => format!("path {route}"),
                };
                build.emit(&def_path, def, "http", label);
            }
            return;
        }
        let target = build.resolve(path, scope, view, 0);
        if let Some((fn_path, function)) = build.function(&target) {
            self.routed.insert((fn_path.clone(), function));
            let methods = api_view_methods(build, &fn_path, function);
            if methods.is_empty() {
                build.emit(&fn_path, function, "http", format!("path {route}"));
            }
            for method in methods {
                build.emit(&fn_path, function, "http", format!("{method} {route}"));
            }
        }
    }

    /// A DRF router's routes under `prefix`.
    fn router(&mut self, build: &mut Build, router: &Val, prefix: &str) {
        self.reached_routers.insert(router.clone());
        let registrations: Vec<_> = self
            .registrations
            .iter()
            .filter(|(r, ..)| r == router)
            .cloned()
            .collect();
        for (_, path, scope, call) in registrations {
            let Some(viewset) = call.param(1, "viewset") else {
                continue;
            };
            let Val::Def(class_path, class) = build.resolve(&path, scope, viewset, 0) else {
                continue;
            };
            let registered = call
                .param(0, "prefix")
                .map(|p| build.piece(&path, scope, p).render())
                .unwrap_or_default();
            let lookup = lookup_field(build, &class_path, class);
            let base = format!("{prefix}{registered}/");
            let detail = format!("{base}<{lookup}>/");
            self.routed.insert((class_path.clone(), class));
            viewset_actions(build, &class_path, class, &base, &detail);
        }
    }

    /// View classes, viewsets and `@api_view` functions no route reached:
    /// the framework still calls them once routed, so they answer with
    /// their own name.
    fn unrouted(&self, build: &mut Build) {
        let all = build.all();
        let mut has_subclass = HashSet::new();
        for (path, summary) in &all {
            for def in summary.defs.iter().filter(|d| d.is_class) {
                for base in &def.bases {
                    if let Val::Def(base_path, base) = build.resolve(path, def.scope, base, 0) {
                        has_subclass.insert((base_path, base));
                    }
                }
            }
        }
        for (path, summary) in &all {
            for (index, def) in summary.defs.iter().enumerate() {
                let key = (path.clone(), index);
                if self.routed.contains(&key) || has_subclass.contains(&key) {
                    continue;
                }
                let name = format!("(unrouted) {}.{}", build.module_name(path), def.qualname);
                if !def.is_class {
                    for method in api_view_methods(build, path, index) {
                        build.emit(path, index, "http", format!("{method} {name}"));
                    }
                    continue;
                }
                if build.inherits(path, index, table::VIEWSET_BASES) {
                    viewset_actions(build, path, index, &format!("{name} "), &name);
                } else if build.inherits(path, index, table::VIEW_BASES) {
                    for (method, def_path, def) in build.view_handlers(path, index) {
                        let method = method.unwrap_or_else(|| "path".to_string());
                        build.emit(&def_path, def, "http", format!("{method} {name}"));
                    }
                }
            }
        }
    }
}

/// The router's standard actions and `@action` methods of a viewset.
fn viewset_actions(build: &mut Build, path: &Path, class: usize, base: &str, detail: &str) {
    for (name, def_path, def) in build.methods(path, class) {
        if let Some((_, method, is_detail)) =
            table::ACTIONS.iter().find(|(action, ..)| *action == name)
        {
            let route = if *is_detail { detail } else { base };
            build.emit(
                &def_path,
                def,
                "http",
                format!("{method} {}", display(route.trim_end())),
            );
            continue;
        }
        let Some(summary) = build.summary(&def_path) else {
            continue;
        };
        let function = &summary.defs[def];
        for decorator in &function.decorators {
            let name = build.external(&def_path, function.scope, decorator.callee());
            if !name.is_some_and(|n| table::EXTRA_ACTION.contains(&n.as_str())) {
                continue;
            }
            let is_detail = matches!(decorator.kwarg("detail"), Some(Ex::Other(v)) if v == "True");
            let url_path = match decorator.kwarg("url_path") {
                Some(Ex::Str(p)) => p.clone(),
                _ => function.name.clone(),
            };
            let methods = match decorator.kwarg("methods") {
                Some(Ex::List(items)) => items
                    .iter()
                    .filter_map(|m| match m {
                        Ex::Str(m) => Some(m.to_uppercase()),
                        _ => None,
                    })
                    .collect(),
                _ => vec!["GET".to_string()],
            };
            let under = if is_detail { detail } else { base };
            for method in methods {
                let route = format!("{under}{url_path}/");
                build.emit(
                    &def_path,
                    def,
                    "http",
                    format!("{method} {}", display(&route)),
                );
            }
        }
    }
}

/// The methods `@api_view([...])` declares; empty when not an API view.
fn api_view_methods(build: &Build, path: &Path, function: usize) -> Vec<String> {
    let Some(summary) = build.summary(path) else {
        return Vec::new();
    };
    let def = &summary.defs[function];
    for decorator in &def.decorators {
        let name = build.external(path, def.scope, decorator.callee());
        if name.is_some_and(|n| table::API_VIEW.contains(&n.as_str())) {
            return match decorator.param(0, "http_method_names") {
                Some(Ex::List(items)) => items
                    .iter()
                    .filter_map(|m| match m {
                        Ex::Str(m) => Some(m.to_uppercase()),
                        _ => None,
                    })
                    .collect(),
                _ => vec!["GET".to_string()],
            };
        }
    }
    Vec::new()
}

/// A viewset's `lookup_field` (`pk` by default).
fn lookup_field(build: &Build, path: &Path, class: usize) -> String {
    match build.class_attr(path, class, "lookup_field", 0) {
        Val::Bound(file, scope, name) => build
            .bound_ex(&file, scope, &name)
            .and_then(|ex| build.eval_str(&file, scope, &ex, 0))
            .unwrap_or_else(|| "pk".to_string()),
        _ => "pk".to_string(),
    }
}

fn display(route: &str) -> String {
    if route.is_empty() {
        "/".to_string()
    } else {
        route.to_string()
    }
}

/// `management/commands/<name>.py`: `manage.py <name>` runs its `Command`.
fn management_commands(build: &mut Build) {
    for (path, summary) in build.all() {
        let relative = build.host.relative_path(&path);
        let Some(at) = relative.find(table::COMMAND_DIR) else {
            continue;
        };
        let file = &relative[at + table::COMMAND_DIR.len()..];
        let Some(command) = file.strip_suffix(".py") else {
            continue;
        };
        if command.contains('/') || command.starts_with('_') {
            continue;
        }
        let Some(class) = summary
            .defs
            .iter()
            .position(|d| d.is_class && d.scope == 0 && d.name == table::COMMAND_CLASS)
        else {
            continue;
        };
        if !build.inherits(&path, class, table::COMMAND_BASES) {
            continue;
        }
        for (name, def_path, def) in build.methods(&path, class) {
            if table::COMMAND_METHODS.contains(&name.as_str()) {
                build.emit(&def_path, def, "cli", format!("manage.py {command}"));
            }
        }
    }
}
