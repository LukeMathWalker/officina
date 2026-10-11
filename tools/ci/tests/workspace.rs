//! Checks the snapshot built from the real officina workspace.

use officina_ci::config::Platform;
use officina_ci::snapshot;

#[test]
fn the_officina_workspace_snapshot_is_accurate() {
    let graph = guppy::MetadataCommand::new()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .build_graph()
        .unwrap();
    // Also validates every package's `[package.metadata.ci]`.
    let ws = snapshot(&graph).unwrap();

    assert_eq!(ws.members["officina-ci"].root, "tools/ci");
    assert_eq!(ws.members["officina-ci"].project(), "tools");
    assert_eq!(
        ws.members["cbindgen-ui-tests"].root,
        "cheadergen/ui-tests/cbindgen"
    );
    assert_eq!(ws.members["cbindgen-ui-tests"].project(), "cheadergen");
    // Transitive dependents, across projects.
    let dependents = &ws.members["tracing_log_error"].dependents;
    for name in ["rustdoc_processor", "rustdoc_resolver", "cheadergen_cli"] {
        assert!(
            dependents.contains(name),
            "{name} depends on tracing_log_error"
        );
    }
    assert!(!dependents.contains("tracing_log_error"));
    assert!(ws.members["biscotti"].has_lib);
    // Third-party packages map to the members depending on them.
    let guppy = ws
        .external
        .iter()
        .find(|(package, _)| package.name == "guppy")
        .map(|(_, dependents)| dependents)
        .unwrap();
    assert!(guppy.contains("officina-ci") && guppy.contains("cargo-px"));
    // CI configuration.
    assert!(
        ws.members["cargo-px"]
            .ci
            .platforms
            .contains(&Platform::Windows)
    );
    assert_eq!(ws.members["biscotti"].ci.platforms, [Platform::Linux]);
}
