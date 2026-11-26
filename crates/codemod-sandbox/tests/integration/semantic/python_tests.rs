//! Python semantic analysis: one test per fixture.

jssg_test! {
    name: one_file,
    fixture: "python/one_file",
    codemod: "py_one_file.js",
    targets: ["input.py"],
    scope: file,
}

jssg_test! {
    name: cross_file,
    fixture: "python/cross_file",
    codemod: "py_cross_file.js",
    targets: ["main.py"],
    scope: workspace,
    preprocess: ["main.py"],
}

// The run tells the provider about no file: references still reach the
// importing module the step is running on.
jssg_test! {
    name: cross_file_with_nothing_preprocessed,
    fixture: "python/cross_file",
    codemod: "py_cross_file.js",
    targets: ["main.py"],
    scope: workspace,
}

jssg_test! {
    name: references_only_through_imports,
    fixture: "python/false_positive_references",
    codemod: "py_false_positive_references.js",
    targets: ["app.py", "models.py"],
    scope: workspace,
    preprocess: ["really_imports.py", "models.py"],
}

jssg_test! {
    name: django_views_services_and_tasks,
    fixture: "python/django_flow",
    codemod: "py_django.js",
    targets: ["shop/services.py", "shop/api.py"],
    scope: workspace,
}

jssg_test! {
    name: fastapi,
    fixture: "python/fastapi_flow",
    codemod: "py_fastapi.js",
    targets: ["app/main.py", "app/repository.py"],
    scope: workspace,
}

jssg_test! {
    name: codemod_sql_taint_to_parameterized_queries,
    fixture: "python/django_flow",
    codemod: "py_sql_taint.js",
    target: "shop/views.py",
    scope: workspace,
    expected_dir: "python/django_flow_expected",
}
