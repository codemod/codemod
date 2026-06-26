use super::ast::{nearest_ancestor, node_text_starts_with, AstNode};

pub(crate) fn gradle_dependency_configuration_for_literal<'a>(
    literal: &AstNode<'a>,
) -> Option<&'a str> {
    if let Some(call) = nearest_ancestor(literal, "call_expression") {
        if let Some(configuration) = gradle_call_configuration(&call) {
            return Some(configuration);
        }
    }

    literal
        .ancestors()
        .filter(|ancestor| ancestor.text().lines().count() == 1)
        .find_map(|ancestor| gradle_call_configuration(&ancestor))
}

pub(crate) fn is_gradle_dependency_configuration(configuration: &str) -> bool {
    GRADLE_DEPENDENCY_CONFIGURATIONS.contains(&configuration)
}

fn gradle_call_configuration<'a>(call: &AstNode<'a>) -> Option<&'a str> {
    GRADLE_DEPENDENCY_CONFIGURATIONS
        .iter()
        .find(|configuration| node_text_starts_with(call, configuration))
        .copied()
}

const GRADLE_DEPENDENCY_CONFIGURATIONS: &[&str] = &[
    "api",
    "implementation",
    "compileOnly",
    "compileOnlyApi",
    "runtimeOnly",
    "testImplementation",
    "testCompileOnly",
    "testRuntimeOnly",
    "annotationProcessor",
    "testAnnotationProcessor",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codemod_steps::utils::ast::ast_grep_root;

    #[test]
    fn detects_kotlin_dependency_configuration() {
        let root = ast_grep_root(
            r#"
dependencies {
    implementation("org.slf4j:slf4j-api:2.0.9")
}
"#,
            "kotlin",
        )
        .unwrap();

        let literal = root
            .root()
            .dfs()
            .find(|node| node.kind() == "string_literal")
            .unwrap();

        assert_eq!(
            gradle_dependency_configuration_for_literal(&literal),
            Some("implementation")
        );
    }

    #[test]
    fn detects_groovy_dependency_configuration() {
        let root = ast_grep_root(
            r#"
dependencies {
    implementation "org.slf4j:slf4j-api:2.0.9"
}
"#,
            "groovy",
        )
        .unwrap();

        let literal = root
            .root()
            .dfs()
            .find(|node| matches!(node.kind().as_ref(), "string_literal" | "string"))
            .unwrap();

        assert_eq!(
            gradle_dependency_configuration_for_literal(&literal),
            Some("implementation")
        );
    }
}
