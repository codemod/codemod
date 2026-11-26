//! What frameworks call, as data. Every name here is fully qualified by the
//! module it is imported from: a decorator or call only counts when the
//! file's imports resolve it to one of these names, never by its spelling.

/// How the pieces of a route or command name join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// `/api` + `/orders` → `/api/orders`.
    Url,
    /// `cli` + `init-db` → `cli init-db`.
    Words,
}

/// Which HTTP method a route answers.
#[derive(Debug, Clone, Copy)]
pub enum Method {
    /// The method the decorator is named after: `@app.get` → `GET`.
    FromName,
    Fixed(&'static str),
    /// A keyword listing methods (`methods=["POST"]`), else the default.
    Keyword(&'static str, &'static str),
}

/// What an entry point's label says.
#[derive(Debug, Clone, Copy)]
pub enum Label {
    /// `GET /orders/{id}`: the path is the argument at `path_arg` (or the
    /// keyword `path_kw`), after the receiver's prefixes.
    Route {
        method: Method,
        path_arg: usize,
        path_kw: &'static str,
    },
    /// The keyword `name` when it is a string, else `module.function`
    /// (Celery's default task name).
    Task,
    /// The command name (argument 0 or `name=`, else the function's name
    /// with `_` as `-`), after the receiver's names.
    Command,
    /// What registers it and its first argument: `exception_handler NotFound`.
    Hook,
    /// The signal(s) in argument 0 and the `sender=` keyword.
    Signal,
}

/// Something that marks the function it decorates or is handed.
#[derive(Debug, Clone, Copy)]
pub struct Marker {
    pub names: &'static [&'static str],
    pub kind: &'static str,
    pub label: Label,
}

/// A call made with a function: `app.add_api_route("/x", handler)`.
#[derive(Debug, Clone, Copy)]
pub struct Registration {
    pub marker: Marker,
    /// Where the function is: a positional index and a keyword.
    pub function_arg: usize,
    pub function_kw: &'static str,
}

/// A call that puts one receiver under another: `app.include_router(r, prefix="/x")`.
#[derive(Debug, Clone, Copy)]
pub struct Mount {
    pub names: &'static [&'static str],
    /// The keyword holding the prefix it is mounted under.
    pub prefix_kw: &'static str,
    /// The prefix given replaces the child's own (Flask's `url_prefix`)
    /// instead of going before it.
    pub replaces_own: bool,
}

/// A framework whose entry points hang off objects it constructs
/// (`FastAPI()`, `Blueprint()`, `Celery()`) or decorators it exports.
#[derive(Debug)]
pub struct Framework {
    pub name: &'static str,
    /// The modules its names are imported from.
    pub modules: &'static [&'static str],
    /// Constructors of receivers, with the keyword (or argument) holding
    /// the receiver's own prefix or name.
    pub receivers: &'static [(&'static str, Option<usize>, &'static str)],
    /// Decorators that make the function they decorate a receiver too
    /// (`@click.group()`), named like a command.
    pub receiver_decorators: &'static [&'static str],
    /// Methods of a receiver that, used as decorators, make the function a
    /// receiver under it (`@cli.group()`).
    pub receiver_methods: &'static [&'static str],
    /// Decorators imported from the framework itself: `@shared_task`.
    pub decorators: &'static [Marker],
    /// Methods of a receiver used as decorators: `@app.get("/x")`.
    pub methods: &'static [Marker],
    /// Methods of a receiver called with a function.
    pub registrations: &'static [Registration],
    pub mounts: &'static [Mount],
    pub join: Join,
}

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

pub const FASTAPI: Framework = Framework {
    name: "FastAPI",
    modules: &["fastapi"],
    receivers: &[
        ("fastapi.FastAPI", None, ""),
        ("fastapi.applications.FastAPI", None, ""),
        ("fastapi.APIRouter", None, "prefix"),
        ("fastapi.routing.APIRouter", None, "prefix"),
    ],
    receiver_decorators: &[],
    receiver_methods: &[],
    decorators: &[],
    methods: &[
        Marker {
            names: HTTP_METHODS,
            kind: "http",
            label: Label::Route {
                method: Method::FromName,
                path_arg: 0,
                path_kw: "path",
            },
        },
        Marker {
            names: &["api_route"],
            kind: "http",
            label: Label::Route {
                method: Method::Keyword("methods", "GET"),
                path_arg: 0,
                path_kw: "path",
            },
        },
        Marker {
            names: &["websocket"],
            kind: "http",
            label: Label::Route {
                method: Method::Fixed("WEBSOCKET"),
                path_arg: 0,
                path_kw: "path",
            },
        },
        Marker {
            names: &["exception_handler", "on_event", "middleware"],
            kind: "hook",
            label: Label::Hook,
        },
    ],
    registrations: &[
        Registration {
            marker: Marker {
                names: &["add_api_route"],
                kind: "http",
                label: Label::Route {
                    method: Method::Keyword("methods", "GET"),
                    path_arg: 0,
                    path_kw: "path",
                },
            },
            function_arg: 1,
            function_kw: "endpoint",
        },
        Registration {
            marker: Marker {
                names: &["add_api_websocket_route"],
                kind: "http",
                label: Label::Route {
                    method: Method::Fixed("WEBSOCKET"),
                    path_arg: 0,
                    path_kw: "path",
                },
            },
            function_arg: 1,
            function_kw: "endpoint",
        },
        Registration {
            marker: Marker {
                names: &["add_exception_handler", "add_event_handler"],
                kind: "hook",
                label: Label::Hook,
            },
            function_arg: 1,
            function_kw: "handler",
        },
    ],
    mounts: &[Mount {
        names: &["include_router"],
        prefix_kw: "prefix",
        replaces_own: false,
    }],
    join: Join::Url,
};

/// `Depends(fn)` and `Security(fn)`: FastAPI calls `fn` to build an argument.
pub const FASTAPI_DEPENDENCIES: &[&str] = &[
    "fastapi.Depends",
    "fastapi.Security",
    "fastapi.params.Depends",
    "fastapi.params.Security",
    "fastapi.param_functions.Depends",
    "fastapi.param_functions.Security",
];

pub const FLASK: Framework = Framework {
    name: "Flask",
    modules: &["flask"],
    receivers: &[
        ("flask.Flask", None, ""),
        ("flask.app.Flask", None, ""),
        ("flask.Blueprint", None, "url_prefix"),
        ("flask.blueprints.Blueprint", None, "url_prefix"),
    ],
    receiver_decorators: &[],
    receiver_methods: &[],
    decorators: &[],
    methods: &[
        Marker {
            names: &["route"],
            kind: "http",
            label: Label::Route {
                method: Method::Keyword("methods", "GET"),
                path_arg: 0,
                path_kw: "rule",
            },
        },
        Marker {
            names: &["get", "post", "put", "patch", "delete"],
            kind: "http",
            label: Label::Route {
                method: Method::FromName,
                path_arg: 0,
                path_kw: "rule",
            },
        },
        Marker {
            names: &[
                "before_request",
                "after_request",
                "teardown_request",
                "errorhandler",
                "context_processor",
                "before_app_request",
                "after_app_request",
                "app_errorhandler",
            ],
            kind: "hook",
            label: Label::Hook,
        },
    ],
    registrations: &[Registration {
        marker: Marker {
            names: &["add_url_rule"],
            kind: "http",
            label: Label::Route {
                method: Method::Keyword("methods", "GET"),
                path_arg: 0,
                path_kw: "rule",
            },
        },
        function_arg: 2,
        function_kw: "view_func",
    }],
    mounts: &[Mount {
        names: &["register_blueprint"],
        prefix_kw: "url_prefix",
        replaces_own: true,
    }],
    join: Join::Url,
};

pub const CELERY: Framework = Framework {
    name: "Celery",
    modules: &["celery"],
    receivers: &[("celery.Celery", None, "")],
    receiver_decorators: &[],
    receiver_methods: &[],
    decorators: &[Marker {
        names: &["celery.shared_task", "celery.app.shared_task"],
        kind: "task",
        label: Label::Task,
    }],
    methods: &[Marker {
        names: &["task"],
        kind: "task",
        label: Label::Task,
    }],
    registrations: &[],
    mounts: &[],
    join: Join::Words,
};

pub const CLICK: Framework = Framework {
    name: "Click",
    modules: &["click"],
    receivers: &[("click.Group", Some(0), "name")],
    receiver_decorators: &["click.group"],
    receiver_methods: &["group"],
    decorators: &[Marker {
        names: &["click.command", "click.group"],
        kind: "cli",
        label: Label::Command,
    }],
    methods: &[Marker {
        names: &["command", "group"],
        kind: "cli",
        label: Label::Command,
    }],
    registrations: &[],
    mounts: &[Mount {
        names: &["add_command"],
        prefix_kw: "name",
        replaces_own: true,
    }],
    join: Join::Words,
};

pub const TYPER: Framework = Framework {
    name: "Typer",
    modules: &["typer"],
    receivers: &[("typer.Typer", None, "name")],
    receiver_decorators: &[],
    receiver_methods: &[],
    decorators: &[],
    methods: &[Marker {
        names: &["command", "callback"],
        kind: "cli",
        label: Label::Command,
    }],
    registrations: &[],
    mounts: &[Mount {
        names: &["add_typer"],
        prefix_kw: "name",
        replaces_own: true,
    }],
    join: Join::Words,
};

pub const DJANGO_SIGNALS: Framework = Framework {
    name: "Django signals",
    modules: &[
        "django.dispatch",
        "django.db.models.signals",
        "django.core.signals",
    ],
    receivers: &[
        ("django.dispatch.Signal", None, ""),
        ("django.dispatch.dispatcher.Signal", None, ""),
    ],
    receiver_decorators: &[],
    receiver_methods: &[],
    decorators: &[Marker {
        names: &[
            "django.dispatch.receiver",
            "django.dispatch.dispatcher.receiver",
        ],
        kind: "signal",
        label: Label::Signal,
    }],
    methods: &[],
    registrations: &[Registration {
        marker: Marker {
            names: &["connect"],
            kind: "signal",
            label: Label::Signal,
        },
        function_arg: 0,
        function_kw: "receiver",
    }],
    mounts: &[],
    join: Join::Words,
};

/// Signals Django itself defines: `post_save.connect(fn)` registers `fn`.
pub const DJANGO_BUILTIN_SIGNALS: &[&str] = &[
    "django.db.models.signals.",
    "django.core.signals.",
    "django.test.signals.",
    "django.contrib.auth.signals.",
];

pub const FRAMEWORKS: &[&Framework] = &[&FASTAPI, &FLASK, &CELERY, &CLICK, &TYPER, &DJANGO_SIGNALS];

/// Django's URL configuration and class-based views (with Django REST
/// framework's routers and viewsets).
pub mod django {
    /// `path(route, view)` and its regex forms.
    pub const PATH: &[&str] = &["django.urls.path", "django.urls.conf.path"];
    pub const REGEX_PATH: &[&str] = &[
        "django.urls.re_path",
        "django.urls.conf.re_path",
        "django.conf.urls.url",
        "django.conf.urls.re_path",
    ];
    pub const INCLUDE: &[&str] = &[
        "django.urls.include",
        "django.urls.conf.include",
        "django.conf.urls.include",
    ];
    /// The settings name of the root URLconf.
    pub const ROOT_URLCONF: &str = "ROOT_URLCONF";
    /// The module-level name a URLconf lists its patterns under.
    pub const URLPATTERNS: &str = "urlpatterns";

    /// DRF routers: `router.register(prefix, ViewSet)`, then `router.urls`.
    pub const ROUTERS: &[&str] = &[
        "rest_framework.routers.DefaultRouter",
        "rest_framework.routers.SimpleRouter",
    ];
    pub const REGISTER: &str = "register";
    pub const ROUTER_URLS: &str = "urls";

    /// Bases (by prefix) whose subclasses are viewsets: the router calls
    /// their actions.
    pub const VIEWSET_BASES: &[&str] = &["rest_framework.viewsets."];
    /// Bases (by prefix) whose subclasses are views answering by HTTP method.
    pub const VIEW_BASES: &[&str] = &[
        "django.views.",
        "rest_framework.views.",
        "rest_framework.generics.",
        "flask.views.",
    ];
    /// Methods a view answers each HTTP method with; `dispatch` answers all.
    pub const HANDLERS: &[&str] = &[
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ];
    pub const DISPATCH: &[&str] = &["dispatch", "dispatch_request"];
    /// What a router routes each viewset action to: (action, method, detail).
    pub const ACTIONS: &[(&str, &str, bool)] = &[
        ("list", "GET", false),
        ("create", "POST", false),
        ("retrieve", "GET", true),
        ("update", "PUT", true),
        ("partial_update", "PATCH", true),
        ("destroy", "DELETE", true),
    ];
    /// `@action(detail=..., methods=[...], url_path=...)` on a viewset method.
    pub const EXTRA_ACTION: &[&str] = &["rest_framework.decorators.action"];
    /// `@api_view(["GET"])` on a function view: the methods it answers.
    pub const API_VIEW: &[&str] = &["rest_framework.decorators.api_view"];

    /// `management/commands/<name>.py` defines `class Command(BaseCommand)`;
    /// `manage.py <name>` calls these methods.
    pub const COMMAND_BASES: &[&str] = &["django.core.management."];
    pub const COMMAND_CLASS: &str = "Command";
    pub const COMMAND_DIR: &str = "management/commands/";
    pub const COMMAND_METHODS: &[&str] = &[
        "handle",
        "add_arguments",
        "handle_label",
        "handle_app_config",
    ];
}

/// Packaging metadata naming a function as a console script or plugin.
pub mod packaging {
    /// (TOML table path, kind). A script's label is its name.
    pub const PYPROJECT_SCRIPTS: &[(&[&str], &str)] = &[
        (&["project", "scripts"], "script"),
        (&["project", "gui-scripts"], "script"),
        (&["tool", "poetry", "scripts"], "script"),
    ];
    /// `[project.entry-points.<group>]`: label `<group>:<name>`.
    pub const PYPROJECT_GROUPS: &[&str] = &["project", "entry-points"];
    /// setup.cfg `[options.entry_points]` groups that are scripts; any
    /// other group is a plugin group.
    pub const SETUP_CFG_SCRIPT_GROUPS: &[&str] = &["console_scripts", "gui_scripts"];
    pub const PLUGIN_KIND: &str = "plugin";
}

/// Methods a framework's base classes call on subclasses: overriding one
/// is how the framework reaches workspace code. (Base class prefixes,
/// method names; a trailing `*` matches any rest.) Kind `hook`, label
/// `<method> of <base>`.
pub const OVERRIDES: &[(&[&str], &[&str])] = &[
    (
        &[
            "rest_framework.views.",
            "rest_framework.generics.",
            "rest_framework.viewsets.",
        ],
        &[
            "get_queryset",
            "get_object",
            "get_serializer",
            "get_serializer_class",
            "get_serializer_context",
            "filter_queryset",
            "paginate_queryset",
            "get_paginated_response",
            "perform_create",
            "perform_update",
            "perform_destroy",
            "get_permissions",
            "get_authenticators",
            "get_throttles",
            "initial",
            "finalize_response",
            "handle_exception",
        ],
    ),
    (
        &["rest_framework.serializers."],
        &[
            "create",
            "update",
            "validate",
            "validate_*",
            "to_representation",
            "to_internal_value",
        ],
    ),
    (
        &["django.views."],
        &[
            "setup",
            "get_queryset",
            "get_object",
            "get_context_data",
            "get_template_names",
            "get_form_class",
            "get_form_kwargs",
            "get_success_url",
            "form_valid",
            "form_invalid",
        ],
    ),
    (&["django.forms."], &["clean", "clean_*"]),
    (
        &["django.contrib.admin."],
        &[
            "get_queryset",
            "save_model",
            "delete_model",
            "get_readonly_fields",
            "has_add_permission",
            "has_change_permission",
            "has_delete_permission",
            "has_view_permission",
        ],
    ),
    (
        &["django.apps.AppConfig", "django.apps.config.AppConfig"],
        &["ready"],
    ),
];

pub const OVERRIDE_KIND: &str = "hook";

/// Running a module: `python -m pkg.mod` and `python script.py`.
pub const MAIN_KIND: &str = "main";
