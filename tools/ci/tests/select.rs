use std::collections::{BTreeMap, BTreeSet};

use officina_ci::{ExternalPackage, Member, Reason, Selection, Snapshot, select};

/// A workspace shaped like a slice of officina:
/// `log_error` <- `processor` <- `cli`, plus an unrelated `cookies`, and
/// `cli` + `cli_tests` in the `gen` project.
fn workspace() -> Snapshot {
    let member = |root: &str, dependents: &[&str], has_lib: bool| Member {
        root: root.to_owned(),
        dependents: dependents.iter().map(|d| d.to_string()).collect(),
        has_lib,
        ..Member::default()
    };
    Snapshot {
        members: BTreeMap::from([
            (
                "log_error".into(),
                member("log_error", &["processor", "cli"], true),
            ),
            (
                "processor".into(),
                member("reflection/processor", &["cli"], true),
            ),
            ("cli".into(), member("gen/cli", &[], false)),
            ("cli_tests".into(), member("gen/ui-tests", &[], true)),
            ("cookies".into(), member("cookies", &[], true)),
            ("cargo-px".into(), member("cargo-px", &[], true)),
        ]),
        external: BTreeMap::from([
            (external("serde", "1.0.0"), set(&["processor", "cli"])),
            (external("time", "0.3.0"), set(&["cookies"])),
        ]),
    }
}

fn external(name: &str, version: &str) -> ExternalPackage {
    ExternalPackage {
        name: name.into(),
        version: version.into(),
        source: "registry+https://github.com/rust-lang/crates.io-index".into(),
    }
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn names(selection: &Selection) -> Vec<&str> {
    selection.packages.keys().map(String::as_str).collect()
}

#[test]
fn a_change_selects_its_package_and_dependents() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["log_error/src/lib.rs"]), None);
    assert_eq!(names(&selection), ["cli", "log_error", "processor"]);
    assert_eq!(selection.everything, None);
    assert_eq!(
        selection.packages["log_error"],
        Reason::Changed("log_error/src/lib.rs".into())
    );
    assert_eq!(
        selection.packages["cli"],
        Reason::DependsOn("log_error".into())
    );
}

#[test]
fn files_are_owned_by_the_most_specific_package() {
    let mut ws = workspace();
    ws.members.insert(
        "fixture".into(),
        Member {
            root: "gen/ui-tests/fixture".into(),
            ..Member::default()
        },
    );
    let selection = select(
        Some(&ws),
        &ws,
        &paths(&["gen/ui-tests/fixture/src/lib.rs"]),
        None,
    );
    assert_eq!(names(&selection), ["fixture"]);
}

#[test]
fn a_package_root_prefix_is_not_ownership() {
    // `cargo-px-extra/...` is not inside `cargo-px`.
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["cargo-px-extra/file"]), None);
    assert!(selection.everything.is_some());
}

#[test]
fn project_files_outside_packages_select_the_whole_project() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["gen/docs/guide.md"]), None);
    assert_eq!(names(&selection), ["cli", "cli_tests"]);
}

#[test]
fn repository_wide_inputs_select_everything() {
    let ws = workspace();
    for path in [
        "Cargo.toml",
        "rust-toolchain.toml",
        ".config/nextest.toml",
        ".github/workflows/ci.yml",
        "tools/ci/src/lib.rs",
    ] {
        let selection = select(Some(&ws), &ws, &paths(&[path]), None);
        assert_eq!(
            selection.everything,
            Some(format!("`{path}` changed")),
            "{path}"
        );
        assert_eq!(selection.packages.len(), ws.members.len(), "{path}");
    }
}

#[test]
fn unknown_files_outside_projects_select_everything() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["deny.toml"]), None);
    assert_eq!(
        selection.everything.as_deref(),
        Some("`deny.toml` is outside every project")
    );
}

#[test]
fn repository_docs_select_nothing() {
    let ws = workspace();
    let selection = select(
        Some(&ws),
        &ws,
        &paths(&[
            "README.md",
            ".github/pull_request_template.md",
            "release-plz.toml",
        ]),
        None,
    );
    assert!(selection.packages.is_empty());
    assert_eq!(selection.everything, None);
}

#[test]
fn lockfile_changes_select_dependents_of_changed_packages() {
    let base = workspace();
    let mut head = workspace();
    let dependents = head.external.remove(&external("serde", "1.0.0")).unwrap();
    head.external.insert(external("serde", "1.0.1"), dependents);
    let selection = select(Some(&base), &head, &paths(&["Cargo.lock"]), None);
    assert_eq!(names(&selection), ["cli", "processor"]);
    assert!(matches!(
        selection.packages["processor"],
        Reason::Lockfile(_)
    ));
}

#[test]
fn lockfile_changes_without_resolution_changes_select_nothing() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["Cargo.lock"]), None);
    assert!(selection.packages.is_empty());
    assert_eq!(selection.everything, None);
}

#[test]
fn removed_packages_select_their_former_dependents() {
    let base = workspace();
    let mut head = workspace();
    head.members.remove("log_error");
    for member in head.members.values_mut() {
        member.dependents.remove("log_error");
    }
    let selection = select(
        Some(&base),
        &head,
        &paths(&["log_error/Cargo.toml", "log_error/src/lib.rs"]),
        None,
    );
    assert_eq!(names(&selection), ["cli", "processor"]);
}

#[test]
fn an_unloadable_base_selects_everything() {
    let ws = workspace();
    let selection = select(None, &ws, &paths(&["README.md"]), None);
    assert!(selection.everything.is_some());
    assert_eq!(selection.packages.len(), ws.members.len());
}

#[test]
fn everything_can_be_requested() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &[], Some("push to main"));
    assert_eq!(selection.everything.as_deref(), Some("push to main"));
    assert_eq!(selection.packages.len(), ws.members.len());
}

#[test]
fn selected_projects_are_listed() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&["log_error/src/lib.rs"]), None);
    assert_eq!(selection.projects, ["gen", "log_error", "reflection"]);
}

#[test]
fn a_project_workflow_selects_only_that_project() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&[".github/workflows/gen.yml"]), None);
    assert_eq!(selection.everything, None);
    assert_eq!(names(&selection), ["cli", "cli_tests"]);
}

#[test]
fn other_workflows_select_everything() {
    let ws = workspace();
    let selection = select(Some(&ws), &ws, &paths(&[".github/workflows/ci.yml"]), None);
    assert!(selection.everything.is_some());
}
