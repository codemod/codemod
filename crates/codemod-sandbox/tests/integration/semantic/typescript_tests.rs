//! TypeScript semantic analysis: one test per fixture.

jssg_test! {
    name: one_file,
    fixture: "typescript/one_file",
    codemod: "ts_one_file.js",
    targets: ["input.ts"],
    scope: file,
}

jssg_test! {
    name: cross_file,
    fixture: "typescript/cross_file",
    codemod: "ts_cross_file.js",
    targets: ["main.ts"],
    scope: workspace,
    preprocess: ["main.ts"],
}

jssg_test! {
    name: reexports,
    fixture: "typescript/reexport_definition",
    codemod: "ts_reexport.js",
    targets: [
        "packages/web/src/app.ts",
        "packages/shared/src/index.ts",
        "packages/shared/src/dates.ts",
    ],
    scope: workspace,
}

jssg_test! {
    name: definition_exports,
    fixture: "typescript/definition_exports",
    codemod: "ts_definition_exports.js",
    targets: ["app.ts"],
    scope: workspace,
}

jssg_test! {
    name: module_top_level_effects,
    fixture: "typescript/side_effect_barrel",
    codemod: "ts_module_effects.js",
    targets: ["src/App.ts", "src/icons/register.ts", "src/icons/Icon.ts"],
    scope: workspace,
}

jssg_test! {
    name: jsx,
    fixture: "typescript/jsx_callees",
    codemod: "tsx_jsx.js",
    targets: ["src/Toolbar.tsx", "src/Nav.tsx"],
    scope: workspace,
}

jssg_test! {
    name: builtin_models,
    fixture: "typescript/builtin_models",
    codemod: "ts_builtin_models.js",
    targets: ["src/models.ts", "src/consumer.ts"],
    scope: workspace,
}

jssg_test! {
    name: workspace_package_without_install,
    fixture: "typescript/workspace_package_no_install",
    codemod: "ts_workspace_package.js",
    targets: ["packages/app/src/App.ts"],
    scope: workspace,
}

jssg_test! {
    name: overloads,
    fixture: "typescript/overloads",
    codemod: "ts_overloads.js",
    targets: ["main.ts"],
    scope: workspace,
}

jssg_test! {
    name: callees_through_returned_values,
    fixture: "typescript/returns",
    codemod: "ts_returns.js",
    targets: ["src/app.ts"],
    scope: workspace,
}

jssg_test! {
    name: interprocedural_flow,
    fixture: "typescript/interprocedural",
    codemod: "ts_interprocedural.js",
    targets: ["app.ts"],
    scope: workspace,
}

jssg_test! {
    name: default_import,
    fixture: "typescript/default_import_refs",
    codemod: "tsx_default_import.js",
    targets: ["emails/WelcomeEmail.tsx"],
    scope: workspace,
}

jssg_test! {
    name: module_resolution,
    fixture: "typescript/module_resolution",
    codemod: "ts_module_resolution.js",
    targets: ["src/pages/home.ts"],
    scope: workspace,
}

jssg_test! {
    name: package_boundary,
    fixture: "typescript/package_boundary",
    codemod: "ts_package_boundary.js",
    targets: ["packages/app/src/App.ts"],
    scope: workspace,
}

jssg_test! {
    name: analyses_as_values,
    fixture: "typescript/analyses",
    codemod: "ts_analyses.js",
    targets: ["client.ts"],
    scope: workspace,
}

jssg_test! {
    name: join_by,
    fixture: "typescript/join_by",
    codemod: "ts_join_by.js",
    targets: ["bus.ts"],
    scope: workspace,
}

// Types from the TypeScript 7 checker (`types: "ts7"`). They need its `tsc`
// binary: CODEMOD_TS_API names it (CI installs the pinned typescript@7.0).
// Without one the test is skipped.
jssg_test! {
    name: types_from_the_checker,
    fixture: "typescript/type_callees",
    codemod: "ts_types.js",
    targets: ["service.ts"],
    scope: ts7,
}
