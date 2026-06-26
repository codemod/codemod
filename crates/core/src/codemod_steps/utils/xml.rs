use std::ops::Range;

use super::ast::AstNode;
use super::ranges::{content_slice, trim_range};

pub(crate) fn xml_element_name(node: &AstNode<'_>) -> Option<String> {
    if node.kind() != "element" {
        return None;
    }
    node.children()
        .find(|child| child.kind() == "STag" || child.kind() == "EmptyElemTag")
        .and_then(|tag| {
            tag.children()
                .find(|child| child.kind() == "Name")
                .map(|name| name.text().to_string())
        })
}

pub(crate) fn xml_direct_child_element<'a>(node: &AstNode<'a>, tag: &str) -> Option<AstNode<'a>> {
    node.children()
        .find(|child| child.kind() == "content")?
        .children()
        .find(|child| child.kind() == "element" && xml_element_name(child).as_deref() == Some(tag))
}

pub(crate) fn xml_direct_child_text(node: &AstNode<'_>, tag: &str) -> Option<String> {
    let element = xml_direct_child_element(node, tag)?;
    xml_element_trimmed_text_range(&element).map(|(text, _)| text)
}

pub(crate) fn xml_element_trimmed_text_range(node: &AstNode<'_>) -> Option<(String, Range<usize>)> {
    let text_node = node
        .children()
        .find(|child| child.kind() == "content")?
        .children()
        .find(|child| child.kind() == "CharData")?;
    let range = trim_range(text_node.text().as_ref(), text_node.range())?;
    Some((
        content_slice(text_node.root().root().text().as_ref(), range.clone())?.to_string(),
        range,
    ))
}

pub(crate) fn xml_element_is_inside(node: &AstNode<'_>, tag_names: &[&str]) -> bool {
    node.ancestors().any(|ancestor| {
        ancestor.kind() == "element"
            && xml_element_name(&ancestor)
                .as_deref()
                .is_some_and(|name| tag_names.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codemod_steps::utils::ast::ast_grep_root;

    #[test]
    fn reads_xml_element_names_and_direct_child_text() {
        let root = ast_grep_root(
            r#"
<project>
  <dependencies>
    <dependency>
      <groupId>org.slf4j</groupId>
      <artifactId>slf4j-api</artifactId>
      <version>
        2.0.9
      </version>
    </dependency>
  </dependencies>
</project>
"#,
            "xml",
        )
        .unwrap();
        let dependency = root
            .root()
            .dfs()
            .find(|node| {
                node.kind() == "element" && xml_element_name(node).as_deref() == Some("dependency")
            })
            .unwrap();

        assert_eq!(xml_element_name(&dependency).as_deref(), Some("dependency"));
        assert_eq!(
            xml_direct_child_text(&dependency, "groupId").as_deref(),
            Some("org.slf4j")
        );
        assert_eq!(
            xml_direct_child_text(&dependency, "artifactId").as_deref(),
            Some("slf4j-api")
        );

        let version = xml_direct_child_element(&dependency, "version").unwrap();
        let (version_text, version_range) = xml_element_trimmed_text_range(&version).unwrap();
        let source = root.root().text();

        assert_eq!(version_text, "2.0.9");
        assert_eq!(&source[version_range], "2.0.9");
    }

    #[test]
    fn detects_xml_element_ancestors() {
        let root = ast_grep_root(
            r#"
<project>
  <dependencyManagement>
    <dependencies>
      <dependency>
        <groupId>org.slf4j</groupId>
        <artifactId>slf4j-api</artifactId>
        <version>2.0.9</version>
      </dependency>
    </dependencies>
  </dependencyManagement>
</project>
"#,
            "xml",
        )
        .unwrap();
        let dependency = root
            .root()
            .dfs()
            .find(|node| {
                node.kind() == "element" && xml_element_name(node).as_deref() == Some("dependency")
            })
            .unwrap();

        assert!(xml_element_is_inside(
            &dependency,
            &["dependencyManagement"]
        ));
        assert!(!xml_element_is_inside(&dependency, &["build"]));
    }
}
