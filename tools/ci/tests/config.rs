use officina_ci::config::{CiConfig, Platform};
use serde_json::json;

#[test]
fn packages_without_a_ci_table_get_the_defaults() {
    let config = CiConfig::from_metadata(&json!(null)).unwrap();
    assert_eq!(config, CiConfig::default());
    assert_eq!(config.platforms, [Platform::Linux]);
    assert!(config.tests);
    let sets: Vec<_> = config
        .feature_sets
        .iter()
        .map(|set| set.name.as_str())
        .collect();
    assert_eq!(sets, ["default", "all", "no-default"]);
    assert_eq!(config.docs_feature_set().args, ["--all-features"]);
}

#[test]
fn fields_override_the_defaults_independently() {
    let config = CiConfig::from_metadata(&json!({
        "ci": {
            "platforms": ["linux", "windows"],
            "feature-sets": [
                { "name": "default" },
                { "name": "unstable", "args": ["--features", "unstable"], "rustflags": "--cfg unstable" },
            ],
            "commands": [{ "name": "readme", "run": "cargo rdme --check" }],
            "tools": ["cargo-rdme"],
        }
    }))
    .unwrap();
    assert_eq!(config.platforms, [Platform::Linux, Platform::Windows]);
    assert!(config.tests);
    assert_eq!(
        config.feature_sets[1].rustflags.as_deref(),
        Some("--cfg unstable")
    );
    // Without `docs = true`, docs use the first feature set.
    assert_eq!(config.docs_feature_set().name, "default");
    assert_eq!(config.commands[0].run, "cargo rdme --check");
    assert_eq!(config.tools, ["cargo-rdme"]);
}

#[test]
fn other_metadata_tables_are_ignored() {
    let config =
        CiConfig::from_metadata(&json!({ "docs": { "rs": { "all-features": true } } })).unwrap();
    assert_eq!(config, CiConfig::default());
}

#[test]
fn invalid_configurations_are_rejected() {
    for (ci, error) in [
        (json!({ "platform": ["linux"] }), "unknown field `platform`"),
        (
            json!({ "platforms": ["freebsd"] }),
            "unknown variant `freebsd`",
        ),
        (json!({ "platforms": [] }), "`platforms` must not be empty"),
        (
            json!({ "feature-sets": [] }),
            "`feature-sets` must not be empty",
        ),
        (
            json!({ "feature-sets": [{ "name": "a" }, { "name": "a" }] }),
            "duplicate feature set `a`",
        ),
        (
            json!({ "feature-sets": [{ "name": "a", "docs": true }, { "name": "b", "docs": true }] }),
            "at most one feature set",
        ),
    ] {
        let message = format!(
            "{:#}",
            CiConfig::from_metadata(&json!({ "ci": ci })).unwrap_err()
        );
        assert!(message.contains(error), "expected `{error}` in `{message}`");
    }
}
