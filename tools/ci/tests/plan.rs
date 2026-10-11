use std::collections::BTreeMap;

use officina_ci::config::{CiConfig, Platform};
use officina_ci::plan::{plan, to_matrix};
use officina_ci::{Member, Reason, Selection, Snapshot};

fn member(root: &str, ci: CiConfig) -> Member {
    Member {
        root: root.into(),
        has_lib: true,
        ci,
        ..Member::default()
    }
}

fn selection(packages: &[&str]) -> Selection {
    Selection {
        everything: None,
        packages: packages
            .iter()
            .map(|name| (name.to_string(), Reason::Changed(String::new())))
            .collect(),
        projects: Vec::new(),
    }
}

fn workspace() -> Snapshot {
    let everywhere = CiConfig {
        platforms: vec![Platform::Linux, Platform::Macos, Platform::Windows],
        ..CiConfig::default()
    };
    let readme = CiConfig {
        tools: vec!["cargo-rdme".into()],
        ..CiConfig::default()
    };
    let untested = CiConfig {
        tests: false,
        ..CiConfig::default()
    };
    Snapshot {
        members: BTreeMap::from([
            ("px".into(), member("px", everywhere.clone())),
            ("px_env".into(), member("px/px_env", CiConfig::default())),
            ("biscotti".into(), member("biscotti", readme)),
            ("gen".into(), member("gen/gen", untested)),
        ]),
        external: BTreeMap::new(),
    }
}

#[test]
fn jobs_group_packages_by_project_and_platform() {
    let jobs = plan(&selection(&["px", "px_env", "biscotti"]), &workspace());
    let summary: Vec<_> = jobs
        .iter()
        .map(|job| (job.name(), job.packages.join(" ")))
        .collect();
    assert_eq!(
        summary,
        [
            ("biscotti".into(), "biscotti".into()),
            ("px".into(), "px px_env".into()),
            ("px (macos)".into(), "px".into()),
            ("px (windows)".into(), "px".into()),
        ]
    );
}

#[test]
fn jobs_install_the_tools_their_checks_need() {
    let jobs = plan(&selection(&["biscotti", "gen", "px"]), &workspace());
    let tools: BTreeMap<_, _> = jobs
        .iter()
        .map(|job| (job.name(), job.tools.join(",")))
        .collect();
    assert_eq!(tools["biscotti"], "cargo-rdme,nextest");
    // `tests = false`: no nextest.
    assert_eq!(tools["gen"], "");
    assert_eq!(tools["px (windows)"], "nextest");
}

#[test]
fn an_empty_selection_has_no_jobs() {
    let jobs = plan(&selection(&[]), &workspace());
    assert!(jobs.is_empty());
    assert_eq!(to_matrix(&jobs), serde_json::json!({ "include": [] }));
}

#[test]
fn matrix_entries_carry_everything_a_job_needs() {
    let jobs = plan(&selection(&["px"]), &workspace());
    assert_eq!(
        to_matrix(&jobs)["include"][2],
        serde_json::json!({
            "name": "px (windows)",
            "project": "px",
            "platform": "windows",
            "runner": "windows-latest",
            "packages": "px",
            "tools": "nextest",
        })
    );
}
