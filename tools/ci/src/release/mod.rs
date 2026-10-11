//! Releases: package, stage, publish, and tag the crates whose version isn't
//! released yet.
//!
//! A release goes through these steps, each consuming the previous one's
//! output, so that nothing is rebuilt along the way:
//!
//! 1. [`plan`]: the crates to release and their GitHub releases.
//! 2. Package: verified `.crate` files, release notes, and pre-flight checks,
//!    recorded in a [`Manifest`]. Binaries are built by cargo-dist.
//! 3. Stage: draft GitHub releases, with their assets.
//! 4. Publish: upload the packaged crates to crates.io.
//! 5. Tag: create the tags, then publish the draft releases.
//!
//! **A tag marks a completed release.** A crate is released if it has no tag
//! for its version and that version is either not on crates.io, or on
//! crates.io with the checksum of our package (i.e. a previous run published
//! it but didn't finish). Versions on crates.io with another checksum were
//! released before this repository existed, and are skipped. Every step is
//! idempotent, so rerunning the release finishes an interrupted one.

pub mod config;
pub mod io;
pub mod steps;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, bail, ensure};
use guppy::graph::PackageGraph;
use guppy::graph::PackagePublish;
use serde::{Deserialize, Serialize};

use config::ReleaseConfig;

/// A crate that can be published to crates.io.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// The package directory, relative to the repository root, with `/` as separator.
    pub dir: String,
    /// Whether cargo-dist builds binaries for it.
    pub dist: bool,
    pub config: ReleaseConfig,
}

impl Package {
    /// The tag marking this version's release.
    pub fn tag(&self) -> String {
        format!("{}-v{}", self.name, self.version)
    }
}

/// The publishable packages of the workspace.
pub fn packages(graph: &PackageGraph) -> anyhow::Result<Vec<Package>> {
    let mut packages = Vec::new();
    for package in graph.workspace().iter() {
        if !package.publish().can_publish_to(PackagePublish::CRATES_IO) {
            continue;
        }
        let metadata = package.metadata_table();
        let dir = package
            .source()
            .workspace_path()
            .context("workspace member without a workspace path")?;
        packages.push(Package {
            name: package.name().to_owned(),
            version: package.version().to_string(),
            dir: dir.as_str().replace('\\', "/"),
            dist: metadata.pointer("/dist/dist") == Some(&serde_json::Value::Bool(true)),
            config: ReleaseConfig::from_metadata(metadata).with_context(|| {
                format!(
                    "invalid `[package.metadata.release]` in `{}`",
                    package.name()
                )
            })?,
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

/// Whether a crate is published already.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CrateState {
    /// Not on crates.io yet.
    New,
    /// On crates.io with our checksum: a previous run published it.
    Published,
}

/// A crate to release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCrate {
    pub name: String,
    pub version: String,
    pub tag: String,
    pub state: CrateState,
}

/// A GitHub release to create.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedRelease {
    pub tag: String,
    pub title: String,
    /// The changelog with the release notes, relative to the repository root.
    pub changelog: String,
    pub version: String,
    /// The crates released in it.
    pub crates: Vec<String>,
    /// The crate whose cargo-dist binaries are attached to it, if any.
    pub dist: Option<String>,
}

/// What to release.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub crates: Vec<PlannedCrate>,
    pub releases: Vec<PlannedRelease>,
    /// Crates that aren't released, and why.
    pub skipped: BTreeMap<String, String>,
}

/// The crates.io checksums of a crate's published versions.
pub type PublishedVersions = BTreeMap<String, String>;

/// Plans the release of `packages`.
///
/// `tags` are the repository's tags, `published` the published versions of
/// each crate, and `checksum` packages a crate (without verification) and
/// returns its SHA-256: it's only called for versions on crates.io without a tag.
pub fn plan(
    packages: &[Package],
    tags: &BTreeSet<String>,
    published: &BTreeMap<String, PublishedVersions>,
    mut checksum: impl FnMut(&Package) -> anyhow::Result<String>,
) -> anyhow::Result<Plan> {
    let mut plan = Plan::default();
    let mut planned = BTreeMap::new();
    for package in packages {
        let tag = package.tag();
        let state = if tags.contains(&tag) {
            plan.skipped
                .insert(package.name.clone(), format!("`{tag}` exists"));
            continue;
        } else {
            match published
                .get(&package.name)
                .and_then(|versions| versions.get(&package.version))
            {
                None => CrateState::New,
                Some(published_checksum) if *published_checksum == checksum(package)? => {
                    CrateState::Published
                }
                Some(_) => {
                    plan.skipped.insert(
                        package.name.clone(),
                        format!(
                            "{} is on crates.io, published from another commit",
                            package.version
                        ),
                    );
                    continue;
                }
            }
        };
        planned.insert(package.name.clone(), package);
        plan.crates.push(PlannedCrate {
            name: package.name.clone(),
            version: package.version.clone(),
            tag,
            state,
        });
    }

    let by_name: BTreeMap<&str, &Package> = packages.iter().map(|p| (p.name.as_str(), p)).collect();
    let mut included_by: BTreeMap<&str, &str> = BTreeMap::new();
    for package in packages {
        for included in &package.config.includes {
            let Some(member) = by_name.get(included.as_str()) else {
                bail!(
                    "`{}` includes `{included}`, which isn't a publishable crate",
                    package.name
                );
            };
            if let Some(other) = included_by.insert(included, &package.name) {
                bail!(
                    "`{included}` is included by both `{other}` and `{}`",
                    package.name
                );
            }
            ensure!(
                member.version == package.version,
                "`{}` includes `{included}`, so they must have the same version ({} != {})",
                package.name,
                package.version,
                member.version
            );
            ensure!(
                planned.contains_key(&package.name) == planned.contains_key(included.as_str()),
                "`{included}` and `{}` share a GitHub release, so they must be released together",
                package.name
            );
            ensure!(
                !member.dist,
                "`{included}` ships binaries, so it needs its own GitHub release (its installers download from its tag)"
            );
        }
    }

    for (name, package) in &planned {
        if included_by.contains_key(name.as_str()) {
            continue;
        }
        let mut crates = vec![name.clone()];
        crates.extend(package.config.includes.iter().cloned());
        let changelog = package
            .config
            .changelog
            .as_deref()
            .unwrap_or("CHANGELOG.md");
        plan.releases.push(PlannedRelease {
            tag: package.tag(),
            title: format!(
                "{} {}",
                package.config.name.as_deref().unwrap_or(&package.name),
                package.version
            ),
            changelog: normalize(&format!("{}/{changelog}", package.dir)),
            version: package.version.clone(),
            crates,
            dist: package.dist.then(|| package.name.clone()),
        });
    }
    Ok(plan)
}

/// Resolves `.` and `..` components in a `/`-separated relative path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// Extracts the release notes of `version` from a Keep a Changelog-style
/// changelog: the body of its `## [version]` (or `## version`) section.
pub fn release_notes(changelog: &str, version: &str) -> anyhow::Result<String> {
    let is_heading = |line: &str| {
        line.strip_prefix("## ").is_some_and(|heading| {
            let heading = heading.trim_start_matches('[');
            heading
                .strip_prefix(version)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([']', ' ']))
        })
    };
    let mut lines = changelog.lines().skip_while(|line| !is_heading(line));
    ensure!(lines.next().is_some(), "no `## [{version}]` section");
    let notes: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    let notes = notes.join("\n").trim().to_owned();
    ensure!(!notes.is_empty(), "the `## [{version}]` section is empty");
    Ok(notes)
}

/// Everything packaging produced: the input of every later step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The commit being released.
    pub commit: String,
    pub crates: Vec<ManifestCrate>,
    pub releases: Vec<ManifestRelease>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCrate {
    pub name: String,
    pub version: String,
    pub tag: String,
    pub state: CrateState,
    /// The packaged crate's file name, in the manifest's directory.
    pub file: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRelease {
    pub tag: String,
    pub title: String,
    pub notes: String,
    pub crates: Vec<String>,
    pub dist: Option<String>,
}
