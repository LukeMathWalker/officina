//! The release steps, as run by `cargo ci release <step>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail, ensure};
use guppy::graph::PackageGraph;

use super::{CrateState, Manifest, ManifestCrate, ManifestRelease, Package, Plan, io};

/// Plans the release of the workspace's crates.
pub fn plan(repository: &Path, graph: &PackageGraph) -> anyhow::Result<(Plan, Vec<Package>)> {
    io::ensure_clean(repository)?;
    let packages = super::packages(graph)?;
    let tags = io::remote_tags(repository)?;
    let mut published = BTreeMap::new();
    for package in &packages {
        published.insert(package.name.clone(), io::published_versions(&package.name)?);
    }
    let target_dir = graph.workspace().target_directory().as_std_path();
    let plan = super::plan(&packages, &tags, &published, |package| {
        let files = io::package(repository, target_dir, &[package], false)?;
        io::sha256(&files[0])
    })?;
    Ok((plan, packages))
}

/// Prints a plan for humans.
pub fn print_plan(plan: &Plan) {
    if plan.crates.is_empty() {
        eprintln!("Nothing to release.");
    } else {
        eprintln!("Releasing {} crate(s):", plan.crates.len());
        for krate in &plan.crates {
            let state = match krate.state {
                CrateState::New => "new",
                CrateState::Published => "already on crates.io",
            };
            eprintln!(
                "  {} {} ({state}), tagged `{}`",
                krate.name, krate.version, krate.tag
            );
        }
        eprintln!("GitHub releases:");
        for release in &plan.releases {
            let binaries = release
                .dist
                .as_ref()
                .map(|_| ", with binaries")
                .unwrap_or_default();
            eprintln!(
                "  {} on `{}` ({}){binaries}",
                release.title,
                release.tag,
                release.crates.join(", ")
            );
        }
    }
    if !plan.skipped.is_empty() {
        eprintln!("Not released:");
        for (name, why) in &plan.skipped {
            eprintln!("  {name}: {why}");
        }
    }
}

/// The cargo-dist build matrix for the plan's binaries: one entry per crate and target.
pub fn dist_matrix(repository: &Path, plan: &Plan) -> anyhow::Result<serde_json::Value> {
    let mut include = Vec::new();
    for release in &plan.releases {
        let Some(krate) = &release.dist else { continue };
        let output = Command::new("dist")
            .args([
                "plan",
                &format!("--tag={}", release.tag),
                "--output-format=json",
            ])
            .current_dir(repository)
            .output()
            .context("failed to run `dist plan`: is cargo-dist installed?")?;
        ensure!(
            output.status.success(),
            "`dist plan --tag={}` failed",
            release.tag
        );
        let dist_plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let entries = dist_plan
            .pointer("/ci/github/artifacts_matrix/include")
            .and_then(|entries| entries.as_array())
            .context("`dist plan` produced no build matrix")?;
        for entry in entries {
            let mut entry = entry.clone();
            entry["crate"] = krate.clone().into();
            entry["tag"] = release.tag.clone().into();
            include.push(entry);
        }
    }
    Ok(serde_json::json!({ "include": include }))
}

/// Packages the planned crates (verified) and writes them, with the manifest,
/// to `out`.
pub fn package(repository: &Path, graph: &PackageGraph, out: &Path) -> anyhow::Result<Manifest> {
    let (plan, packages) = plan(repository, graph)?;
    print_plan(&plan);
    let commit = io::head_commit(repository)?;

    // Release notes first: a missing changelog section is the likeliest mistake.
    let mut releases = Vec::new();
    for release in &plan.releases {
        let path = repository.join(&release.changelog);
        let changelog = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to read `{}` for {}",
                release.changelog, release.title
            )
        })?;
        let notes = super::release_notes(&changelog, &release.version).with_context(|| {
            format!(
                "no release notes for {} in `{}`",
                release.title, release.changelog
            )
        })?;
        releases.push(ManifestRelease {
            tag: release.tag.clone(),
            title: release.title.clone(),
            notes,
            crates: release.crates.clone(),
            dist: release.dist.clone(),
        });
    }

    std::fs::create_dir_all(out)?;
    let mut crates = Vec::new();
    if !plan.crates.is_empty() {
        let planned: Vec<&Package> = plan
            .crates
            .iter()
            .map(|krate| {
                packages
                    .iter()
                    .find(|p| p.name == krate.name)
                    .expect("planned crates are packages")
            })
            .collect();
        let target_dir = graph.workspace().target_directory().as_std_path();
        // Verified: cargo builds each packaged crate, against the other planned crates.
        let files = io::package(repository, target_dir, &planned, true)?;
        for (krate, file) in plan.crates.iter().zip(files) {
            let name = io::crate_file(&krate.name, &krate.version);
            std::fs::copy(&file, out.join(&name))
                .with_context(|| format!("failed to copy `{}`", file.display()))?;
            crates.push(ManifestCrate {
                name: krate.name.clone(),
                version: krate.version.clone(),
                tag: krate.tag.clone(),
                state: krate.state,
                file: name,
                sha256: io::sha256(&file)?,
            });
        }
    }
    let manifest = Manifest {
        commit,
        crates,
        releases,
    };
    std::fs::write(
        out.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    eprintln!(
        "Wrote {} crate(s) and the manifest to `{}`.",
        manifest.crates.len(),
        out.display()
    );
    Ok(manifest)
}

/// Reads a manifest written by [`package`].
pub fn read_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let manifest = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    serde_json::from_str(&manifest)
        .with_context(|| format!("invalid manifest `{}`", path.display()))
}

/// Creates or updates the draft GitHub releases, with their assets: the
/// cargo-dist artifacts of crate `<name>` are read from `<assets>/<name>/`.
///
/// With `dry_run`, only checks and reports what it would do.
pub fn stage(
    repository: &Path,
    manifest: &Manifest,
    assets: &Path,
    dry_run: bool,
) -> anyhow::Result<()> {
    let existing = io::github_releases(repository)?;
    for release in &manifest.releases {
        let files = match &release.dist {
            None => Vec::new(),
            Some(krate) => {
                let dir = assets.join(krate);
                ensure!(
                    dir.is_dir(),
                    "no binaries for {}: `{}` doesn't exist",
                    release.title,
                    dir.display()
                );
                let files = files_in(&dir)?;
                ensure!(
                    !files.is_empty(),
                    "no binaries for {} in `{}`",
                    release.title,
                    dir.display()
                );
                files
            }
        };
        let current = existing.iter().find(|r| r.tag_name == release.tag);
        if current.is_some_and(|r| !r.draft) {
            bail!(
                "a published GitHub release already exists for `{}`",
                release.tag
            );
        }
        if dry_run {
            let action = if current.is_some() {
                "update"
            } else {
                "create"
            };
            eprintln!(
                "Would {action} the draft of {} ({} asset(s)).",
                release.title,
                files.len()
            );
            continue;
        }
        io::upsert_draft(repository, release, &manifest.commit, current.is_some())?;
        io::upload_assets(repository, &release.tag, &files)?;
        eprintln!("Staged {} ({} asset(s)).", release.title, files.len());
    }
    Ok(())
}

/// The files directly inside `dir`, sorted.
fn files_in(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read `{}`", dir.display()))?
    {
        let path = entry?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Publishes the manifest's crates to crates.io.
///
/// Crates already published with the manifest's checksum are skipped. The
/// others are repackaged, without building them, and compared byte for byte
/// with the manifest before anything is uploaded. With `dry_run`, runs
/// `cargo publish --dry-run` instead of uploading.
pub fn publish(
    repository: &Path,
    graph: &PackageGraph,
    manifest: &Manifest,
    dry_run: bool,
) -> anyhow::Result<()> {
    ensure!(
        io::head_commit(repository)? == manifest.commit,
        "the checkout isn't the commit the manifest was packaged from ({})",
        manifest.commit
    );
    let mut pending = Vec::new();
    for krate in &manifest.crates {
        match io::published_versions(&krate.name)?.get(&krate.version) {
            Some(checksum) if *checksum == krate.sha256 => {
                eprintln!("{} {} is already published.", krate.name, krate.version);
            }
            Some(checksum) => bail!(
                "{} {} is on crates.io with checksum {checksum}, not {}",
                krate.name,
                krate.version,
                krate.sha256
            ),
            None => pending.push(krate),
        }
    }
    if pending.is_empty() {
        return Ok(());
    }

    let packages = super::packages(graph)?;
    let pending_packages: Vec<&Package> = pending
        .iter()
        .map(|krate| {
            packages
                .iter()
                .find(|p| p.name == krate.name && p.version == krate.version)
                .with_context(|| format!("{} {} isn't in the workspace", krate.name, krate.version))
        })
        .collect::<anyhow::Result<_>>()?;
    let target_dir = graph.workspace().target_directory().as_std_path();
    let files = io::package(repository, target_dir, &pending_packages, false)?;
    for (krate, file) in pending.iter().zip(&files) {
        let checksum = io::sha256(file)?;
        ensure!(
            checksum == krate.sha256,
            "repackaging {} {} gave checksum {checksum}, not {}: refusing to publish",
            krate.name,
            krate.version,
            krate.sha256
        );
    }

    let names: Vec<&str> = pending.iter().map(|krate| krate.name.as_str()).collect();
    io::publish(repository, &names, dry_run)?;
    if dry_run {
        eprintln!("Would publish {} crate(s).", pending.len());
        return Ok(());
    }
    for krate in &pending {
        let published = io::published_versions(&krate.name)?;
        ensure!(
            published.get(&krate.version) == Some(&krate.sha256),
            "crates.io doesn't list {} {} with checksum {}",
            krate.name,
            krate.version,
            krate.sha256
        );
    }
    eprintln!("Published {} crate(s).", pending.len());
    Ok(())
}

/// Creates the tags on the manifest's commit, then publishes the draft releases.
///
/// With `dry_run`, only reports what it would do.
pub fn tag(repository: &Path, manifest: &Manifest, dry_run: bool) -> anyhow::Result<()> {
    if dry_run {
        for krate in &manifest.crates {
            eprintln!("Would tag `{}` on {}.", krate.tag, manifest.commit);
        }
        for release in &manifest.releases {
            eprintln!("Would publish {}.", release.title);
        }
        return Ok(());
    }
    for krate in &manifest.crates {
        io::create_tag(repository, &krate.tag, &manifest.commit)?;
        eprintln!("Tagged `{}`.", krate.tag);
    }
    let existing = io::github_releases(repository)?;
    for release in &manifest.releases {
        match existing.iter().find(|r| r.tag_name == release.tag) {
            Some(current) if !current.draft => eprintln!("{} is already published.", release.title),
            Some(_) => {
                io::publish_release(repository, &release.tag)?;
                eprintln!("Published {}.", release.title);
            }
            None => bail!(
                "no draft GitHub release for `{}`: run the stage step first",
                release.tag
            ),
        }
    }
    Ok(())
}
