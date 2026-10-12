use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use officina_ci::release::config::ReleaseConfig;
use officina_ci::release::{CrateState, Package, PublishedVersions, plan, release_notes};

fn package(name: &str, version: &str) -> Package {
    Package {
        name: name.into(),
        version: version.into(),
        dir: format!("project/{name}"),
        dist: false,
        config: ReleaseConfig::default(),
    }
}

fn published(entries: &[(&str, &str, &str)]) -> BTreeMap<String, PublishedVersions> {
    let mut published: BTreeMap<String, PublishedVersions> = BTreeMap::new();
    for (name, version, checksum) in entries {
        published
            .entry(name.to_string())
            .or_default()
            .insert(version.to_string(), checksum.to_string());
    }
    published
}

fn tags(tags: &[&str]) -> BTreeSet<String> {
    tags.iter().map(|tag| tag.to_string()).collect()
}

#[test]
fn crates_are_classified_by_tag_and_crates_io_state() {
    let packages = [
        package("new", "1.0.0"),
        package("tagged", "1.0.0"),
        package("resumed", "1.0.0"),
        package("historical", "1.0.0"),
    ];
    let published = published(&[
        ("tagged", "1.0.0", "aaa"),
        ("resumed", "1.0.0", "ours"),
        ("historical", "1.0.0", "theirs"),
        ("new", "0.9.0", "old"),
    ]);
    let checksummed = RefCell::new(Vec::new());
    let plan = plan(
        &packages,
        &tags(&["tagged-v1.0.0"]),
        &published,
        |package| {
            checksummed.borrow_mut().push(package.name.clone());
            Ok("ours".into())
        },
    )
    .unwrap();

    let crates: Vec<_> = plan
        .crates
        .iter()
        .map(|c| (c.name.as_str(), c.state))
        .collect();
    assert_eq!(
        crates,
        [("new", CrateState::New), ("resumed", CrateState::Published)]
    );
    assert_eq!(plan.crates[0].tag, "new-v1.0.0");
    assert_eq!(plan.skipped["tagged"], "`tagged-v1.0.0` exists");
    assert!(plan.skipped["historical"].contains("published from another commit"));
    // Only versions on crates.io without a tag need packaging to compare checksums.
    assert_eq!(*checksummed.borrow(), ["resumed", "historical"]);
}

#[test]
fn every_crate_gets_its_own_release_by_default() {
    let mut cli = package("cli", "0.2.0");
    cli.dist = true;
    let plan = plan(
        &[cli, package("lib", "1.0.0")],
        &tags(&[]),
        &published(&[]),
        |_| unreachable!(),
    )
    .unwrap();
    let releases: Vec<_> = plan
        .releases
        .iter()
        .map(|r| {
            (
                r.tag.as_str(),
                r.title.as_str(),
                r.changelog.as_str(),
                r.dist.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        releases,
        [
            (
                "cli-v0.2.0",
                "cli 0.2.0",
                "project/cli/CHANGELOG.md",
                Some("cli")
            ),
            ("lib-v1.0.0", "lib 1.0.0", "project/lib/CHANGELOG.md", None),
        ]
    );
}

fn group() -> Vec<Package> {
    let mut cli = package("gen_cli", "0.3.0");
    cli.dist = true;
    cli.config = ReleaseConfig {
        name: Some("gen".into()),
        changelog: Some("../CHANGELOG.md".into()),
        includes: vec!["gen".into(), "gen_macros".into()],
    };
    vec![cli, package("gen", "0.3.0"), package("gen_macros", "0.3.0")]
}

#[test]
fn grouped_crates_share_the_including_crates_release() {
    let plan = plan(&group(), &tags(&[]), &published(&[]), |_| unreachable!()).unwrap();
    assert_eq!(plan.crates.len(), 3, "every crate is still tagged");
    assert_eq!(plan.releases.len(), 1);
    let release = &plan.releases[0];
    assert_eq!(release.tag, "gen_cli-v0.3.0");
    assert_eq!(release.title, "gen 0.3.0");
    assert_eq!(release.changelog, "project/CHANGELOG.md");
    assert_eq!(release.crates, ["gen_cli", "gen", "gen_macros"]);
    assert_eq!(release.dist.as_deref(), Some("gen_cli"));
}

#[test]
fn invalid_groups_are_rejected() {
    let check = |packages: Vec<Package>, tags_: &[&str], expected: &str| {
        let error = plan(&packages, &tags(tags_), &published(&[]), |_| {
            Ok(String::new())
        })
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(expected),
            "expected `{expected}` in `{message}`"
        );
    };
    // A grouped crate released without the others.
    check(group(), &["gen-v0.3.0"], "must be released together");
    // Versions must match.
    let mut packages = group();
    packages[2].version = "0.3.1".into();
    check(packages, &[], "must have the same version");
    // A crate shipping binaries needs its own release.
    let mut packages = group();
    packages[1].dist = true;
    check(packages, &[], "needs its own GitHub release");
    // Unknown crates, and crates included twice.
    let mut packages = group();
    packages[0].config.includes.push("nope".into());
    check(packages, &[], "isn't a publishable crate");
    let mut packages = group();
    packages[1].config.includes.push("gen_macros".into());
    check(packages, &[], "is included by both");
}

const CHANGELOG: &str = "\
# Changelog

## [Unreleased]

## [1.0.10](https://example.com/compare) - 2026-10-11

### Fixed

- Something else.

## [1.0.1](https://example.com/compare) - 2026-10-10

### ⚙️ Other

- A change.

## 1.0.0

## [0.9.0] - 2026-01-01

- Initial release.
";

#[test]
fn release_notes_are_the_versions_section() {
    assert_eq!(
        release_notes(CHANGELOG, "1.0.1").unwrap(),
        "### ⚙️ Other\n\n- A change."
    );
    assert_eq!(
        release_notes(CHANGELOG, "1.0.10").unwrap(),
        "### Fixed\n\n- Something else."
    );
    assert_eq!(
        release_notes(CHANGELOG, "0.9.0").unwrap(),
        "- Initial release."
    );
}

#[test]
fn missing_or_empty_release_notes_are_errors() {
    assert!(
        release_notes(CHANGELOG, "2.0.0")
            .unwrap_err()
            .to_string()
            .contains("no `## [2.0.0]` section")
    );
    assert!(
        release_notes(CHANGELOG, "1.0.0")
            .unwrap_err()
            .to_string()
            .contains("is empty")
    );
    // `1.0` must not match `1.0.1` or `1.0.10`.
    assert!(release_notes(CHANGELOG, "1.0").is_err());
}
