//! Selects the workspace packages that CI must check for a change.
//!
//! The selection is conservative: when in doubt, it runs everything.
//!
//! A change selects:
//!
//! - every package, if it touches a repository-wide input (see [`RUN_EVERYTHING`]);
//! - every package of a project, if it touches the project's own workflow
//!   (`.github/workflows/<project>.yml`);
//! - the package that owns each changed file;
//! - every package of a project, for files in the project's directory that
//!   don't belong to any of its packages (e.g. `cheadergen/docs/`);
//! - the packages that depend on a third-party package whose resolved version
//!   changed in `Cargo.lock`;
//! - every workspace package that depends, directly or transitively, on a
//!   selected package.
//!
//! Files outside every project select nothing if they are listed in
//! [`RUN_NOTHING`], and everything otherwise.
//!
//! The selection logic in [`select`] works on [`Snapshot`]s, a minimal model
//! of the workspace at a revision, so it can be tested without Cargo or Git.
//! [`snapshot`] builds one from a [`guppy`] package graph.

use std::collections::{BTreeMap, BTreeSet};

use guppy::graph::{BuildTargetId, DependencyDirection, PackageGraph, PackageSource};

use crate::config::CiConfig;

/// Paths whose changes can affect every package: they select everything.
///
/// Entries ending with `/` match every path under that directory.
pub const RUN_EVERYTHING: &[&str] = &[
    "Cargo.toml",
    "rust-toolchain.toml",
    ".cargo/",
    ".config/",
    ".github/actions/",
    ".github/workflows/",
    // A selector bug could hide failures: validate its changes against everything.
    "tools/ci/",
];

/// Paths outside every project that don't affect any build or test.
///
/// Entries ending with `/` match every path under that directory.
pub const RUN_NOTHING: &[&str] = &[
    ".gitignore",
    ".github/pull_request_template.md",
    "LICENSE-APACHE",
    "LICENSE-MIT",
    "README.md",
];

/// The workspace at a given revision, reduced to what the selection needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Workspace members, by package name.
    pub members: BTreeMap<String, Member>,
    /// Third-party packages in the resolved dependency graph, each mapped to
    /// the workspace members that depend on it, directly or transitively.
    pub external: BTreeMap<ExternalPackage, BTreeSet<String>>,
}

/// A workspace member.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    /// The directory containing the member's manifest, relative to the
    /// repository root, with `/` as separator.
    pub root: String,
    /// The other members that depend on this one, directly or transitively,
    /// including through dev-dependencies.
    pub dependents: BTreeSet<String>,
    /// Whether the member has a library target (and therefore docs and doctests).
    pub has_lib: bool,
    /// The member's `[package.metadata.ci]` configuration.
    pub ci: CiConfig,
}

impl Member {
    /// The project the member belongs to: the top-level directory containing it.
    pub fn project(&self) -> &str {
        project_of(&self.root).unwrap_or(&self.root)
    }
}

/// A third-party package, identified as `Cargo.lock` identifies it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalPackage {
    pub name: String,
    pub version: String,
    pub source: String,
}

impl std::fmt::Display for ExternalPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl Snapshot {
    /// The member whose directory most specifically contains `path`, if any.
    fn owner(&self, path: &str) -> Option<&str> {
        self.members
            .iter()
            .filter(|(_, member)| is_within(path, &member.root))
            .max_by_key(|(_, member)| member.root.len())
            .map(|(name, _)| name.as_str())
    }

    /// The members inside the top-level directory containing `path`, if `path`
    /// is nested in a directory.
    fn project_members(&self, path: &str) -> impl Iterator<Item = &str> {
        self.members_of(project_of(path))
    }

    /// The members of `project`, if any.
    fn members_of(&self, project: Option<&str>) -> impl Iterator<Item = &str> {
        self.members
            .iter()
            .filter(move |(_, member)| project.is_some_and(|p| member.project() == p))
            .map(|(name, _)| name.as_str())
    }
}

/// Why a package was selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Everything was selected.
    Everything(String),
    /// A file belonging to the package (or its project) changed.
    Changed(String),
    /// A third-party package it depends on changed in `Cargo.lock`.
    Lockfile(ExternalPackage),
    /// It depends on another selected package.
    DependsOn(String),
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reason::Everything(why) => write!(f, "everything selected: {why}"),
            Reason::Changed(path) => write!(f, "`{path}` changed"),
            Reason::Lockfile(package) => write!(f, "`{package}` changed in Cargo.lock"),
            Reason::DependsOn(name) => write!(f, "depends on `{name}`"),
        }
    }
}

/// The packages CI must check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Why everything was selected, if it was.
    pub everything: Option<String>,
    /// The selected packages, each with the first reason it was selected for.
    pub packages: BTreeMap<String, Reason>,
    /// The projects (top-level directories) containing selected packages.
    pub projects: Vec<String>,
}

/// Selects the packages affected by `changed_paths` between `base` and `head`.
///
/// `base` is `None` if the workspace couldn't be loaded at the base revision,
/// in which case everything is selected. `run_everything` forces a full
/// selection, with the given reason.
pub fn select(
    base: Option<&Snapshot>,
    head: &Snapshot,
    changed_paths: &[String],
    run_everything: Option<&str>,
) -> Selection {
    let mut everything = run_everything.map(str::to_owned);
    let mut selected: BTreeMap<String, Reason> = BTreeMap::new();

    match base {
        None => {
            everything.get_or_insert_with(|| {
                "the workspace could not be loaded at the base revision".into()
            });
        }
        Some(base) => {
            for path in changed_paths {
                let workflow_project = path
                    .strip_prefix(".github/workflows/")
                    .and_then(|file| file.strip_suffix(".yml"));
                let project: BTreeSet<&str> = head
                    .members_of(workflow_project)
                    .chain(base.members_of(workflow_project))
                    .collect();
                if !project.is_empty() {
                    for name in project {
                        add(&mut selected, name, Reason::Changed(path.clone()));
                    }
                    continue;
                }
                if matches_any(path, RUN_EVERYTHING) {
                    everything.get_or_insert_with(|| format!("`{path}` changed"));
                    continue;
                }
                let owners: BTreeSet<&str> = [head.owner(path), base.owner(path)]
                    .into_iter()
                    .flatten()
                    .collect();
                let owners = if owners.is_empty() {
                    head.project_members(path)
                        .chain(base.project_members(path))
                        .collect()
                } else {
                    owners
                };
                for name in &owners {
                    add(&mut selected, name, Reason::Changed(path.clone()));
                }
                if owners.is_empty() && path != "Cargo.lock" && !matches_any(path, RUN_NOTHING) {
                    everything.get_or_insert_with(|| format!("`{path}` is outside every project"));
                }
            }
            if changed_paths.iter().any(|path| path == "Cargo.lock") {
                for (a, b) in [(head, base), (base, head)] {
                    for (package, dependents) in &a.external {
                        if !b.external.contains_key(package) {
                            for name in dependents {
                                add(&mut selected, name, Reason::Lockfile(package.clone()));
                            }
                        }
                    }
                }
            }
            let directly_selected: Vec<String> = selected.keys().cloned().collect();
            for name in directly_selected {
                for snapshot in [head, base] {
                    for dependent in snapshot
                        .members
                        .get(&name)
                        .iter()
                        .flat_map(|m| &m.dependents)
                    {
                        add(&mut selected, dependent, Reason::DependsOn(name.clone()));
                    }
                }
            }
        }
    }

    let packages: BTreeMap<String, Reason> = match &everything {
        Some(why) => head
            .members
            .keys()
            .map(|name| (name.clone(), Reason::Everything(why.clone())))
            .collect(),
        // Packages that no longer exist can't be checked; their dependents were selected above.
        None => selected
            .into_iter()
            .filter(|(name, _)| head.members.contains_key(name))
            .collect(),
    };
    let projects: BTreeSet<String> = packages
        .keys()
        .map(|name| head.members[name].project().to_owned())
        .collect();
    Selection {
        everything,
        packages,
        projects: projects.into_iter().collect(),
    }
}

/// Builds a [`Snapshot`] from the package graph of a workspace rooted at the
/// repository root.
pub fn snapshot(graph: &PackageGraph) -> anyhow::Result<Snapshot> {
    let workspace_members = |ids: Vec<&guppy::PackageId>,
                             exclude: Option<&guppy::PackageId>|
     -> anyhow::Result<BTreeSet<String>> {
        Ok(graph
            .query_reverse(ids)?
            .resolve()
            .packages(DependencyDirection::Reverse)
            .filter(|package| package.in_workspace() && Some(package.id()) != exclude)
            .map(|package| package.name().to_owned())
            .collect())
    };

    let mut members = BTreeMap::new();
    for package in graph.workspace().iter() {
        let root = package.source().workspace_path().ok_or_else(|| {
            anyhow::anyhow!(
                "workspace member `{}` has no workspace path",
                package.name()
            )
        })?;
        members.insert(
            package.name().to_owned(),
            Member {
                root: root.as_str().replace('\\', "/"),
                dependents: workspace_members(vec![package.id()], Some(package.id()))?,
                has_lib: package
                    .build_targets()
                    .any(|target| target.id() == BuildTargetId::Library),
                ci: CiConfig::from_metadata(package.metadata_table()).map_err(|error| {
                    anyhow::anyhow!(
                        "invalid `[package.metadata.ci]` in `{}`: {error:#}",
                        package.name()
                    )
                })?,
            },
        );
    }

    let mut external = BTreeMap::new();
    for package in graph.packages().filter(|package| !package.in_workspace()) {
        let source = match package.source() {
            PackageSource::External(source) => source.to_owned(),
            PackageSource::Path(path) => format!("path+{path}"),
            PackageSource::Workspace(_) => continue,
        };
        external.insert(
            ExternalPackage {
                name: package.name().to_owned(),
                version: package.version().to_string(),
                source,
            },
            workspace_members(vec![package.id()], None)?,
        );
    }

    Ok(Snapshot { members, external })
}

/// Selects `name` for `reason`, unless it was already selected for another reason.
fn add(selected: &mut BTreeMap<String, Reason>, name: &str, reason: Reason) {
    selected.entry(name.to_owned()).or_insert(reason);
}

/// The top-level directory containing `path`, if `path` is nested in one.
fn project_of(path: &str) -> Option<&str> {
    path.split_once('/').map(|(project, _)| project)
}

/// Whether `path` is `dir` or nested inside it.
fn is_within(path: &str, dir: &str) -> bool {
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path` matches one of `rules`: exact paths, or directories ending with `/`.
fn matches_any(path: &str, rules: &[&str]) -> bool {
    rules.iter().any(|rule| match rule.strip_suffix('/') {
        Some(dir) => is_within(path, dir),
        None => path == *rule,
    })
}
