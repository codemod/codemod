//! Built-in Python entry points: why a function is called from outside the
//! workspace's code. Each test is one rule.

use language_core::{
    ByteRange, CallGraphDirection, CallGraphOptions, EntryPointRule, SemanticProvider,
};
use language_python::RuffSemanticProvider;
use tempfile::TempDir;

/// A workspace of `files`, and a provider over it.
fn workspace(files: &[(&str, &str)]) -> (TempDir, RuffSemanticProvider) {
    let dir = TempDir::new().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let provider = RuffSemanticProvider::workspace_scope(dir.path().to_path_buf());
    (dir, provider)
}

/// The range of the name in the first `def <name>(` of `file`.
fn def_range(dir: &TempDir, file: &str, name: &str) -> (std::path::PathBuf, ByteRange) {
    let path = dir.path().join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let start = text.find(&format!("def {name}(")).expect("no such def") + 4;
    (
        path,
        ByteRange::new(start as u32, (start + name.len()) as u32),
    )
}

/// `kind label` for each entry point of the function `name` in `file`.
fn entry_points_with(
    provider: &RuffSemanticProvider,
    dir: &TempDir,
    file: &str,
    name: &str,
) -> Vec<String> {
    let (path, range) = def_range(dir, file, name);
    provider
        .get_entry_points(&path, range)
        .unwrap()
        .into_iter()
        .map(|e| format!("{} {}", e.kind, e.label))
        .collect()
}

fn entry_points(files: &[(&str, &str)], file: &str, name: &str) -> Vec<String> {
    let (dir, provider) = workspace(files);
    entry_points_with(&provider, &dir, file, name)
}

const NO_ENTRY_POINTS: &[String] = &[];

// ---- packaging ------------------------------------------------------------

#[test]
fn a_project_script_is_a_script_entry_point_named_after_the_script() {
    let found = entry_points(
        &[
            (
                "pyproject.toml",
                "[project]\nname = \"tool\"\n[project.scripts]\ntool-run = \"tool.cli:main\"\n",
            ),
            ("src/tool/__init__.py", ""),
            ("src/tool/cli.py", "def main():\n    pass\n"),
        ],
        "src/tool/cli.py",
        "main",
    );
    assert_eq!(found, ["script tool-run"]);
}

#[test]
fn a_project_entry_point_group_is_a_plugin_labelled_group_and_name() {
    let found = entry_points(
        &[
            (
                "pyproject.toml",
                "[project.entry-points.\"pytest11\"]\nmine = \"plug.hooks:setup\"\n",
            ),
            ("plug/__init__.py", ""),
            ("plug/hooks.py", "def setup():\n    pass\n"),
        ],
        "plug/hooks.py",
        "setup",
    );
    assert_eq!(found, ["plugin pytest11:mine"]);
}

#[test]
fn a_poetry_script_naming_a_class_method_marks_the_method() {
    let found = entry_points(
        &[
            (
                "pyproject.toml",
                "[tool.poetry.scripts]\nserve = \"app.server:Server.run\"\n",
            ),
            ("app/__init__.py", ""),
            (
                "app/server.py",
                "class Server:\n    def run(self):\n        pass\n",
            ),
        ],
        "app/server.py",
        "run",
    );
    assert_eq!(found, ["script serve"]);
}

#[test]
fn a_setup_cfg_console_script_is_a_script_entry_point() {
    let found = entry_points(
        &[
            ("setup.cfg", "[metadata]\nname = tool\n\n[options.entry_points]\nconsole_scripts =\n    tool = tool.cli:main\n"),
            ("tool/__init__.py", ""),
            ("tool/cli.py", "def main():\n    pass\n"),
        ],
        "tool/cli.py",
        "main",
    );
    assert_eq!(found, ["script tool"]);
}

#[test]
fn an_entry_point_adds_no_callers() {
    let (dir, provider) = workspace(&[
        (
            "pyproject.toml",
            "[project.scripts]\ntool = \"tool.cli:main\"\n",
        ),
        ("tool/__init__.py", ""),
        ("tool/cli.py", "def main():\n    pass\n"),
    ]);
    let (path, range) = def_range(&dir, "tool/cli.py", "main");
    let callers = provider
        .get_call_graph(
            &path,
            range,
            CallGraphOptions {
                direction: CallGraphDirection::Callers,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(callers.files.is_empty());
    assert_eq!(provider.get_entry_points(&path, range).unwrap().len(), 1);
}

// ---- running a module -----------------------------------------------------

#[test]
fn a_function_called_from_the_main_guard_is_a_main_entry_point_of_its_module() {
    let found = entry_points(
        &[
            ("pkg/__init__.py", ""),
            (
                "pkg/cli.py",
                "import sys\n\ndef main():\n    return 0\n\nif __name__ == \"__main__\":\n    sys.exit(main())\n",
            ),
        ],
        "pkg/cli.py",
        "main",
    );
    assert_eq!(found, ["main python -m pkg.cli"]);
}

#[test]
fn a_script_outside_any_package_runs_as_python_and_its_path() {
    let found = entry_points(
        &[(
            "manage.py",
            "def main():\n    pass\n\nif __name__ == '__main__':\n    main()\n",
        )],
        "manage.py",
        "main",
    );
    assert_eq!(found, ["main python manage.py"]);
}

#[test]
fn a_function_called_at_the_top_level_of_dunder_main_is_a_main_entry_point_of_the_package() {
    let found = entry_points(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/app.py", "def run():\n    pass\n"),
            ("pkg/__main__.py", "from pkg.app import run\n\nrun()\n"),
        ],
        "pkg/app.py",
        "run",
    );
    assert_eq!(found, ["main python -m pkg"]);
}

#[test]
fn a_function_only_defined_next_to_a_main_guard_is_not_an_entry_point() {
    let found = entry_points(
        &[(
            "tool.py",
            "def helper():\n    pass\n\ndef main():\n    pass\n\nif __name__ == '__main__':\n    main()\n",
        )],
        "tool.py",
        "helper",
    );
    assert_eq!(found, NO_ENTRY_POINTS);
}

// ---- Django ---------------------------------------------------------------

const DJANGO_ROOT: (&str, &str) = ("site/settings.py", "ROOT_URLCONF = \"site.urls\"\n");
const DJANGO_ROOT_URLS: (&str, &str) = (
    "site/urls.py",
    "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"shop.urls\")),\n]\n",
);

#[test]
fn a_django_view_function_is_an_http_entry_point_under_the_including_route() {
    let found = entry_points(
        &[
            ("site/__init__.py", ""),
            DJANGO_ROOT,
            DJANGO_ROOT_URLS,
            ("shop/__init__.py", ""),
            (
                "shop/urls.py",
                "from django.urls import path\nfrom . import views\n\nurlpatterns = [path(\"orders/<int:pk>/\", views.order)]\n",
            ),
            ("shop/views.py", "def order(request, pk):\n    pass\n"),
        ],
        "shop/views.py",
        "order",
    );
    assert_eq!(found, ["http path api/orders/<int:pk>/"]);
}

#[test]
fn a_regex_route_is_shown_without_its_anchors() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            (
                "shop/urls.py",
                "from django.conf.urls import url\nfrom shop.views import health\n\nurlpatterns = [url(r\"^health/$\", health)]\n",
            ),
            ("shop/views.py", "def health(request):\n    pass\n"),
        ],
        "shop/views.py",
        "health",
    );
    assert_eq!(found, ["http path health/"]);
}

#[test]
fn an_as_view_class_answers_with_its_http_method_handlers() {
    let files = [
        ("shop/__init__.py", ""),
        (
            "shop/urls.py",
            "from django.urls import path\nfrom shop.views import Me\n\nurlpatterns = [path(\"me/\", Me.as_view())]\n",
        ),
        (
            "shop/views.py",
            "from rest_framework.views import APIView\n\nclass Me(APIView):\n    def get(self, request):\n        pass\n\n    def patch(self, request):\n        pass\n",
        ),
    ];
    let (dir, provider) = workspace(&files);
    assert_eq!(
        entry_points_with(&provider, &dir, "shop/views.py", "get"),
        ["http GET me/"]
    );
    assert_eq!(
        entry_points_with(&provider, &dir, "shop/views.py", "patch"),
        ["http PATCH me/"]
    );
}

const VIEWSET_URLS: (&str, &str) = (
    "shop/urls.py",
    "from rest_framework.routers import DefaultRouter\nfrom shop import views\n\nrouter = DefaultRouter()\nrouter.register(\"orders\", views.OrderViewSet)\n\nurlpatterns = [] + router.urls\n",
);

#[test]
fn a_router_registered_viewset_routes_its_standard_actions() {
    let files = [
        ("shop/__init__.py", ""),
        VIEWSET_URLS,
        (
            "shop/views.py",
            "from rest_framework import viewsets\n\nclass OrderViewSet(viewsets.ModelViewSet):\n    def list(self, request):\n        pass\n\n    def retrieve(self, request, pk=None):\n        pass\n",
        ),
    ];
    let (dir, provider) = workspace(&files);
    assert_eq!(
        entry_points_with(&provider, &dir, "shop/views.py", "list"),
        ["http GET orders/"]
    );
    assert_eq!(
        entry_points_with(&provider, &dir, "shop/views.py", "retrieve"),
        ["http GET orders/<pk>/"]
    );
}

#[test]
fn a_viewset_action_method_is_routed_under_the_detail_route() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            VIEWSET_URLS,
            (
                "shop/views.py",
                "from rest_framework import viewsets\nfrom rest_framework.decorators import action\n\nclass OrderViewSet(viewsets.ModelViewSet):\n    @action(detail=True, methods=[\"post\"])\n    def cancel(self, request, pk=None):\n        pass\n",
            ),
        ],
        "shop/views.py",
        "cancel",
    );
    assert_eq!(found, ["http POST orders/<pk>/cancel/"]);
}

#[test]
fn a_view_class_no_route_reaches_is_marked_unrouted() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            (
                "shop/views.py",
                "from django.views import View\n\nclass Ping(View):\n    def get(self, request):\n        pass\n",
            ),
        ],
        "shop/views.py",
        "get",
    );
    assert_eq!(found, ["http GET (unrouted) shop.views.Ping"]);
}

#[test]
fn a_management_command_handle_is_a_cli_entry_point() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            ("shop/management/__init__.py", ""),
            ("shop/management/commands/__init__.py", ""),
            (
                "shop/management/commands/reindex.py",
                "from django.core.management.base import BaseCommand\n\nclass Command(BaseCommand):\n    def handle(self, *args, **options):\n        pass\n",
            ),
        ],
        "shop/management/commands/reindex.py",
        "handle",
    );
    assert_eq!(found, ["cli manage.py reindex"]);
}

#[test]
fn a_django_receiver_is_a_signal_entry_point() {
    let found = entry_points(
        &[(
            "signals.py",
            "from django.db.models.signals import post_save\nfrom django.dispatch import receiver\nfrom models import Order\n\n@receiver(post_save, sender=Order)\ndef on_save(sender, instance, **kwargs):\n    pass\n",
        )],
        "signals.py",
        "on_save",
    );
    assert_eq!(found, ["signal post_save sender=Order"]);
}

#[test]
fn connecting_to_a_django_signal_makes_a_signal_entry_point() {
    let found = entry_points(
        &[(
            "signals.py",
            "from django.db.models import signals\n\ndef on_delete(sender, **kwargs):\n    pass\n\nsignals.post_delete.connect(on_delete)\n",
        )],
        "signals.py",
        "on_delete",
    );
    assert_eq!(found, ["signal post_delete"]);
}

#[test]
fn an_overridden_framework_hook_is_a_hook_entry_point() {
    let found = entry_points(
        &[(
            "views.py",
            "from rest_framework import generics\n\nclass Orders(generics.ListAPIView):\n    def get_queryset(self):\n        pass\n",
        )],
        "views.py",
        "get_queryset",
    );
    assert_eq!(
        found,
        ["hook get_queryset of rest_framework.generics.ListAPIView"]
    );
}

// ---- FastAPI --------------------------------------------------------------

#[test]
fn a_fastapi_router_get_handler_is_an_http_entry_point_under_the_router_prefix() {
    let found = entry_points(
        &[(
            "orders.py",
            "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/orders\")\n\n@router.get(\"/{order_id}\")\ndef get_order(order_id: int):\n    pass\n",
        )],
        "orders.py",
        "get_order",
    );
    assert_eq!(found, ["http GET /orders/{order_id}"]);
}

#[test]
fn include_router_prefixes_go_before_the_route() {
    let found = entry_points(
        &[
            ("app/__init__.py", ""),
            (
                "app/orders.py",
                "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n@router.post(\"\")\ndef create_order():\n    pass\n",
            ),
            (
                "app/main.py",
                "from fastapi import FastAPI\nfrom app.orders import router as orders\n\nPREFIX = \"/api\"\napp = FastAPI()\napp.include_router(orders, prefix=PREFIX + \"/orders\")\n",
            ),
        ],
        "app/orders.py",
        "create_order",
    );
    assert_eq!(found, ["http POST /api/orders"]);
}

#[test]
fn add_api_route_registers_an_http_entry_point() {
    let found = entry_points(
        &[(
            "main.py",
            "from fastapi import FastAPI\n\ndef health():\n    pass\n\napp = FastAPI()\napp.add_api_route(\"/health\", health, methods=[\"GET\", \"HEAD\"])\n",
        )],
        "main.py",
        "health",
    );
    assert_eq!(found, ["http GET /health", "http HEAD /health"]);
}

#[test]
fn a_depends_argument_is_a_dependency_entry_point() {
    let found = entry_points(
        &[(
            "api.py",
            "from fastapi import Depends, FastAPI\n\napp = FastAPI()\n\ndef current_user():\n    pass\n\n@app.get(\"/me\")\ndef me(user=Depends(current_user)):\n    pass\n",
        )],
        "api.py",
        "current_user",
    );
    assert_eq!(found, ["dependency Depends api:me"]);
}

#[test]
fn a_decorator_named_get_on_an_unrelated_object_is_not_an_entry_point() {
    let found = entry_points(
        &[(
            "cache.py",
            "class Cache:\n    def get(self, key):\n        return lambda f: f\n\ncache = Cache()\n\n@cache.get(\"/x\")\ndef compute():\n    pass\n",
        )],
        "cache.py",
        "compute",
    );
    assert_eq!(found, NO_ENTRY_POINTS);
}

#[test]
fn a_router_from_a_module_that_is_not_fastapi_is_not_a_receiver() {
    let found = entry_points(
        &[(
            "orders.py",
            "from mylib.routing import APIRouter\n\nrouter = APIRouter()\n\n@router.get(\"/orders\")\ndef list_orders():\n    pass\n",
        )],
        "orders.py",
        "list_orders",
    );
    assert_eq!(found, NO_ENTRY_POINTS);
}

// ---- Flask ----------------------------------------------------------------

#[test]
fn a_blueprint_route_is_prefixed_by_the_blueprints_url_prefix() {
    let found = entry_points(
        &[(
            "shop.py",
            "from flask import Blueprint\n\nbp = Blueprint(\"shop\", __name__, url_prefix=\"/shop\")\n\n@bp.route(\"/cart\", methods=[\"GET\", \"POST\"])\ndef cart():\n    pass\n",
        )],
        "shop.py",
        "cart",
    );
    assert_eq!(found, ["http GET /shop/cart", "http POST /shop/cart"]);
}

#[test]
fn register_blueprint_url_prefix_replaces_the_blueprints_own() {
    let found = entry_points(
        &[
            ("web/__init__.py", ""),
            (
                "web/shop.py",
                "from flask import Blueprint\n\nbp = Blueprint(\"shop\", __name__, url_prefix=\"/shop\")\n\n@bp.get(\"/cart\")\ndef cart():\n    pass\n",
            ),
            (
                "web/app.py",
                "from flask import Flask\nfrom web.shop import bp\n\ndef create_app():\n    app = Flask(__name__)\n    app.register_blueprint(bp, url_prefix=\"/store\")\n    return app\n",
            ),
        ],
        "web/shop.py",
        "cart",
    );
    assert_eq!(found, ["http GET /store/cart"]);
}

#[test]
fn add_url_rule_registers_the_view_function() {
    let found = entry_points(
        &[(
            "app.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\ndef index():\n    pass\n\napp.add_url_rule(\"/\", \"index\", index)\n",
        )],
        "app.py",
        "index",
    );
    assert_eq!(found, ["http GET /"]);
}

// ---- Celery ---------------------------------------------------------------

#[test]
fn a_shared_task_is_a_task_named_after_its_module() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            (
                "shop/tasks.py",
                "from celery import shared_task\n\n@shared_task\ndef reindex():\n    pass\n",
            ),
        ],
        "shop/tasks.py",
        "reindex",
    );
    assert_eq!(found, ["task shop.tasks.reindex"]);
}

#[test]
fn a_task_of_a_celery_app_from_another_module_is_a_task_with_its_given_name() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            ("shop/worker.py", "from celery import Celery\n\ncelery_app = Celery(\"shop\")\n"),
            (
                "shop/tasks.py",
                "from shop.worker import celery_app\n\n@celery_app.task(name=\"shop.sync\", bind=True)\ndef sync(self):\n    pass\n",
            ),
        ],
        "shop/tasks.py",
        "sync",
    );
    assert_eq!(found, ["task shop.sync"]);
}

#[test]
fn a_shared_task_decorator_defined_in_the_workspace_is_not_celerys() {
    let found = entry_points(
        &[(
            "tasks.py",
            "def shared_task(f):\n    return f\n\n@shared_task\ndef reindex():\n    pass\n",
        )],
        "tasks.py",
        "reindex",
    );
    assert_eq!(found, NO_ENTRY_POINTS);
}

#[test]
fn a_package_module_named_like_a_library_does_not_shadow_the_library() {
    let found = entry_points(
        &[
            ("shop/__init__.py", ""),
            ("shop/celery.py", "app = None\n"),
            (
                "shop/tasks.py",
                "from celery import shared_task\n\n@shared_task\ndef reindex():\n    pass\n",
            ),
        ],
        "shop/tasks.py",
        "reindex",
    );
    assert_eq!(found, ["task shop.tasks.reindex"]);
}

// ---- Click and Typer -------------------------------------------------------

#[test]
fn a_click_command_in_a_group_is_labelled_with_the_group() {
    let found = entry_points(
        &[(
            "cli.py",
            "import click\n\n@click.group()\ndef cli():\n    pass\n\n@cli.command()\ndef init_db():\n    pass\n",
        )],
        "cli.py",
        "init_db",
    );
    assert_eq!(found, ["cli click cli init-db"]);
}

#[test]
fn a_typer_command_is_a_cli_entry_point() {
    let found = entry_points(
        &[(
            "cli.py",
            "import typer\n\napp = typer.Typer()\n\n@app.command(\"sync\")\ndef sync_all():\n    pass\n",
        )],
        "cli.py",
        "sync_all",
    );
    assert_eq!(found, ["cli typer sync"]);
}

// ---- declared rules -------------------------------------------------------

#[test]
fn declared_rules_add_to_built_in_entry_points() {
    let (dir, provider) = workspace(&[(
        "tasks.py",
        "from celery import shared_task\n\n@shared_task\ndef reindex():\n    pass\n",
    )]);
    provider.set_entry_point_rules(vec![EntryPointRule {
        decorators: vec!["shared_task".into()],
        kind: "job".into(),
        ..Default::default()
    }]);
    assert_eq!(
        entry_points_with(&provider, &dir, "tasks.py", "reindex"),
        ["job @shared_task tasks.py:reindex", "task tasks.reindex"]
    );
}
