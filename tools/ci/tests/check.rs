use std::collections::BTreeMap;

use officina_ci::check::steps;
use officina_ci::config::{CiConfig, CustomCommand, FeatureSet, Platform};
use officina_ci::{Member, Snapshot};

fn workspace(ci: CiConfig, has_lib: bool) -> Snapshot {
    Snapshot {
        members: BTreeMap::from([(
            "pkg".into(),
            Member {
                root: "project/pkg".into(),
                has_lib,
                ci,
                ..Member::default()
            },
        )]),
        external: BTreeMap::new(),
    }
}

fn names(ws: &Snapshot, platform: Platform) -> Vec<String> {
    steps(ws, &["pkg".into()], platform)
        .into_iter()
        .map(|step| step.name)
        .collect()
}

#[test]
fn default_checks_on_linux() {
    let ws = workspace(CiConfig::default(), true);
    assert_eq!(
        names(&ws, Platform::Linux),
        [
            "fmt",
            "clippy (default)",
            "tests (default)",
            "clippy (all)",
            "tests (all)",
            "clippy (no-default)",
            "tests (no-default)",
            "doctests (all)",
            "docs (all)",
        ]
    );
    let steps = steps(&ws, &["pkg".into()], Platform::Linux);
    assert_eq!(
        steps[3].to_string(),
        "cargo clippy --locked --package pkg --all-features --all-targets -- -D warnings"
    );
    assert_eq!(steps[8].env["RUSTDOCFLAGS"], "-D warnings");
}

#[test]
fn other_platforms_only_lint_and_test() {
    let ci = CiConfig {
        platforms: vec![Platform::Linux, Platform::Windows],
        ..CiConfig::default()
    };
    let ws = workspace(ci, true);
    assert_eq!(
        names(&ws, Platform::Windows),
        [
            "clippy (default)",
            "tests (default)",
            "clippy (all)",
            "tests (all)",
            "clippy (no-default)",
            "tests (no-default)",
        ]
    );
}

#[test]
fn unsupported_platforms_have_no_steps() {
    let ws = workspace(CiConfig::default(), true);
    assert!(names(&ws, Platform::Macos).is_empty());
}

#[test]
fn binaries_have_no_docs_and_untested_packages_no_tests() {
    let ci = CiConfig {
        tests: false,
        ..CiConfig::default()
    };
    let ws = workspace(ci, false);
    assert_eq!(
        names(&ws, Platform::Linux),
        [
            "fmt",
            "clippy (default)",
            "clippy (all)",
            "clippy (no-default)"
        ]
    );
}

#[test]
fn rustflags_get_their_own_target_directory_and_reach_rustdoc() {
    let ci = CiConfig {
        feature_sets: vec![FeatureSet {
            name: "unstable".into(),
            args: vec!["--features".into(), "a,b".into()],
            rustflags: Some("--cfg tracing_unstable".into()),
            docs: false,
        }],
        ..CiConfig::default()
    };
    let ws = workspace(ci, true);
    let steps = steps(&ws, &["pkg".into()], Platform::Linux);
    let by_name: BTreeMap<_, _> = steps
        .iter()
        .map(|step| (step.name.as_str(), step))
        .collect();
    let clippy = by_name["clippy (unstable)"];
    assert_eq!(clippy.env["RUSTFLAGS"], "--cfg tracing_unstable");
    assert_eq!(
        clippy.env["CARGO_TARGET_DIR"],
        "target/ci/cfg-tracing-unstable"
    );
    assert!(
        clippy
            .args
            .windows(2)
            .any(|pair| pair == ["--features", "a,b"])
    );
    assert_eq!(
        by_name["doctests (unstable)"].env["RUSTDOCFLAGS"],
        "--cfg tracing_unstable"
    );
    assert_eq!(
        by_name["docs (unstable)"].env["RUSTDOCFLAGS"],
        "-D warnings --cfg tracing_unstable"
    );
    assert!(!by_name["fmt"].env.contains_key("RUSTFLAGS"));
}

#[test]
fn custom_commands_run_in_the_package_directory_on_linux() {
    let ci = CiConfig {
        platforms: vec![Platform::Linux, Platform::Macos],
        commands: vec![CustomCommand {
            name: "readme".into(),
            run: "cargo rdme --check".into(),
            rustflags: None,
        }],
        ..CiConfig::default()
    };
    let ws = workspace(ci, false);
    let steps = steps(&ws, &["pkg".into()], Platform::Linux);
    let readme = steps.last().unwrap();
    assert_eq!(
        (readme.name.as_str(), readme.dir.as_str()),
        ("readme", "project/pkg")
    );
    assert_eq!(readme.args, ["-c", "cargo rdme --check"]);
    assert!(!names(&ws, Platform::Macos).contains(&"readme".to_string()));
}
