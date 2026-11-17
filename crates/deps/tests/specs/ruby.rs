use super::*;

const GEMFILE_LOCK: &str = r#"
PATH
  remote: .
  specs:
    app (0.3.0)
      rack (>= 2)

PATH
  remote: engines/admin
  specs:
    admin (1.0.0)
      rack

GIT
  remote: https://github.com/rails/rails-html-sanitizer.git
  revision: 9a1c3b5e0b2f6a8d4c7e1f0a3b5c7d9e1f2a4b6c
  specs:
    rails-html-sanitizer (1.6.0)
      nokogiri (~> 1.14)

GEM
  remote: https://rubygems.org/
  specs:
    minitest (5.25.1)
    nokogiri (1.16.0-arm64-darwin)
      racc (~> 1.4)
    nokogiri (1.16.0-x86_64-linux)
      racc (~> 1.4)
    racc (1.8.1)
    rack (3.2.8)
    rake (13.4.2)
    rspec (3.13.2)

PLATFORMS
  arm64-darwin
  x86_64-linux

DEPENDENCIES
  admin!
  app!
  minitest
  nokogiri (>= 1.15, < 2)
  rack (~> 3.0)
  rails-html-sanitizer!
  rake (~> 13.0)
  rspec (~> 3.12)

CHECKSUMS
  rack (3.2.8) sha256=1fecc85e463ebc7a2a945fff829ffcd549babeb1556384796fad946c28dc4ec7

BUNDLED WITH
   2.5.22
"#;

const GEMFILE: &str = r#"
source "https://rubygems.org"
gemspec

gem "rack", "~> 3.0"
gem "admin", path: "engines/admin"
gem "nokogiri", ">= 1.15", "< 2"
gem "rails-html-sanitizer", git: "https://github.com/rails/rails-html-sanitizer.git"

group :development, :test do
  gem "rspec", "~> 3.12"
end

gem "minitest", group: :test, require: false
"#;

fn bundle() -> Graph {
    lockfiles::gemfile_lock(GEMFILE_LOCK, &|path| {
        (path == "Gemfile").then(|| GEMFILE.to_string())
    })
    .unwrap()
}

fn bundle_dependencies(kinds: &[Kind]) -> Vec<String> {
    let walk = Walk::new(bundle());
    let root = walk.nodes_named("<.>")[0];
    described(&walk, walk.dependencies(root, kinds))
}

#[test]
fn the_bundle_depends_on_each_gemfile_lock_dependency_with_its_requirement_and_git_or_path_gems_with_none(
) {
    assert_eq!(
        bundle_dependencies(&[Kind::Prod]),
        [
            "<.>@0.0.0-use.local prod admin@1.0.0 ?",
            "<.>@0.0.0-use.local prod app@0.3.0 ?",
            "<.>@0.0.0-use.local prod nokogiri@1.16.0 >= 1.15, < 2",
            "<.>@0.0.0-use.local prod rack@3.2.8 ~> 3.0",
            "<.>@0.0.0-use.local prod rails-html-sanitizer@1.6.0 ?"
        ]
    );
}

#[test]
fn development_and_test_gems_and_the_gemspec_development_dependencies_are_dev() {
    assert_eq!(
        bundle_dependencies(&[Kind::Dev]),
        [
            "<.>@0.0.0-use.local dev minitest@5.25.1 >= 0",
            "<.>@0.0.0-use.local dev rake@13.4.2 ~> 13.0",
            "<.>@0.0.0-use.local dev rspec@3.13.2 ~> 3.12"
        ]
    );
}

#[test]
fn a_gem_locked_for_several_platforms_is_one_node_at_its_version() {
    let walk = Walk::new(bundle());
    assert_eq!(
        described(&walk, walk.declaring("racc", &[])),
        ["nokogiri@1.16.0 prod racc@1.8.1 ~> 1.4"]
    );
}

#[test]
fn path_gems_and_the_bundle_are_workspaces_in_their_directories() {
    let graph = bundle();
    let workspaces: Vec<_> = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(id, node)| Some((graph.label(id), node.workspace.clone()?)))
        .collect();
    assert_eq!(
        workspaces,
        [
            ("app@0.3.0".to_string(), ".".to_string()),
            ("admin@1.0.0".to_string(), "engines/admin".to_string()),
            ("<.>@0.0.0-use.local".to_string(), ".".to_string())
        ]
    );
}

#[test]
fn a_gemfile_lock_is_found_and_read_with_the_gemfile_beside_it() {
    let dir = repository(&[("Gemfile.lock", GEMFILE_LOCK), ("Gemfile", GEMFILE)]);
    let walk = Walk::new(lockfiles::load(dir.path(), &[]).unwrap());
    assert_eq!(
        described(&walk, walk.declaring("rspec", &[])),
        ["<.>@0.0.0-use.local dev rspec@3.13.2 ~> 3.12"]
    );
}

#[test]
fn a_file_without_gemfile_lock_sections_is_not_read() {
    assert!(lockfiles::gemfile_lock("# not a lockfile\n", &|_| None).is_err());
}

#[test]
fn a_pessimistic_requirement_allows_only_the_last_given_part_to_rise() {
    use codemod_deps::semver::satisfies;
    let check = |range, version| satisfies(Ecosystem::Rubygems, range, version);
    assert_eq!(
        [
            check("~> 2.2", "2.9.1"),
            check("~> 2.2", "3.0"),
            check("~> 2.2.0", "2.2.5"),
            check("~> 2.2.0", "2.3.0"),
            check(">= 1.15, < 2", "1.16.0"),
        ],
        [Some(true), Some(false), Some(true), Some(false), Some(true)]
    );
}

#[test]
fn a_gem_prerelease_sorts_before_its_release_and_trailing_zeros_do_not_count() {
    use codemod_deps::semver::{compare, is_prerelease};
    use std::cmp::Ordering;
    assert_eq!(
        (
            compare(Ecosystem::Rubygems, "1.0.0.rc1", "1.0.0"),
            compare(Ecosystem::Rubygems, "1.0", "1.0.0"),
            compare(Ecosystem::Rubygems, "1.10", "1.9"),
            is_prerelease(Ecosystem::Rubygems, "2.0.0.beta1"),
        ),
        (Ordering::Less, Ordering::Equal, Ordering::Greater, true)
    );
}

#[test]
fn a_string_that_is_not_a_gem_version_sorts_before_every_version() {
    use codemod_deps::semver::compare;
    let mut versions = ["1.10.0", "main", "1.2.0", "HEAD"];
    versions.sort_by(|a, b| compare(Ecosystem::Rubygems, a, b));
    assert_eq!(versions, ["HEAD", "main", "1.2.0", "1.10.0"]);
}

#[test]
fn a_gem_requirement_that_rubygems_cannot_read_is_unknown() {
    use codemod_deps::semver::satisfies;
    assert_eq!(satisfies(Ecosystem::Rubygems, "^1.2", "1.2.0"), None);
}

const COMPACT_INDEX: &str = "---
1.15.5-x86_64-linux racc:~> 1.4|checksum:c5d9,ruby:>= 2.7
1.15.5 mini_portile2:~> 2.8.2,racc:~> 1.4|checksum:2244,ruby:>= 2.7.0
1.16.0.rc1 racc:>= 1.4&< 2|checksum:9a1c
";

fn indexed_dependencies() -> Vec<(String, Vec<String>)> {
    use codemod_deps::registry::rubygems_releases;
    rubygems_releases(COMPACT_INDEX)
        .unwrap()
        .into_iter()
        .map(|release| {
            let dependencies = release
                .dependencies
                .iter()
                .map(|(name, range, _)| format!("{name} {range}"))
                .collect();
            (release.version, dependencies)
        })
        .collect()
}

#[test]
fn a_gem_built_for_several_platforms_is_one_release_with_its_plain_ruby_dependencies() {
    assert_eq!(
        indexed_dependencies()[0],
        (
            "1.15.5".to_string(),
            vec![
                "mini_portile2 ~> 2.8.2".to_string(),
                "racc ~> 1.4".to_string()
            ]
        )
    );
}

#[test]
fn compact_index_requirements_joined_by_ampersands_read_as_one_requirement() {
    assert_eq!(
        indexed_dependencies()[1],
        (
            "1.16.0.rc1".to_string(),
            vec!["racc >= 1.4, < 2".to_string()]
        )
    );
}
