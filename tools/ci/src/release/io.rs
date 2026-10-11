//! The release steps' interactions with crates.io, Git, Cargo and GitHub.
//!
//! GitHub operations go through the `gh` CLI, which resolves the repository
//! from the checkout and authenticates with `GH_TOKEN`.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{ManifestRelease, Package, PublishedVersions};

/// Runs `program` with `args` in `dir`, returning its stdout.
fn run(dir: &Path, program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to run `{program}`"))?;
    if !output.status.success() {
        bail!("`{program} {}` failed", args.join(" "));
    }
    String::from_utf8(output.stdout).context("non-UTF-8 output")
}

/// The published versions of `name` and their checksums, from crates.io's
/// sparse index.
pub fn published_versions(name: &str) -> anyhow::Result<PublishedVersions> {
    let lower = name.to_lowercase();
    let prefix = match lower.len() {
        1 => "1".to_owned(),
        2 => "2".to_owned(),
        3 => format!("3/{}", &lower[..1]),
        _ => format!("{}/{}", &lower[..2], &lower[2..4]),
    };
    let url = format!("https://index.crates.io/{prefix}/{lower}");
    let output = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--write-out",
            "\n%{http_code}",
            &url,
        ])
        .output()
        .context("failed to run `curl`")?;
    let body = String::from_utf8(output.stdout).context("non-UTF-8 index entry")?;
    let (body, status) = body.rsplit_once('\n').context("unexpected `curl` output")?;
    match status {
        "200" => {}
        // Never published.
        "404" | "403" => return Ok(PublishedVersions::new()),
        _ => bail!("fetching {url} failed with status {status}"),
    }
    #[derive(Deserialize)]
    struct Entry {
        vers: String,
        cksum: String,
    }
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: Entry = serde_json::from_str(line)
                .with_context(|| format!("invalid index entry for `{name}`"))?;
            Ok((entry.vers, entry.cksum))
        })
        .collect()
}

/// The tags of the `origin` remote.
pub fn remote_tags(repository: &Path) -> anyhow::Result<BTreeSet<String>> {
    let output = run(
        repository,
        "git",
        &["ls-remote", "--tags", "--refs", "origin"],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| line.split_once("refs/tags/").map(|(_, tag)| tag.to_owned()))
        .collect())
}

/// Fails if `repository` has uncommitted changes: packaged crates record the
/// commit they come from, so they must be built from a clean checkout.
pub fn ensure_clean(repository: &Path) -> anyhow::Result<()> {
    let status = run(
        repository,
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !status.trim().is_empty() {
        bail!(
            "releases are packaged from a clean checkout, but there are uncommitted changes:\n{status}"
        );
    }
    Ok(())
}

/// The commit checked out in `repository`.
pub fn head_commit(repository: &Path) -> anyhow::Result<String> {
    Ok(run(repository, "git", &["rev-parse", "HEAD"])?
        .trim()
        .to_owned())
}

/// Packages `packages` with `cargo package --locked`, returning the `.crate`
/// files. Without `verify`, packages aren't built (no code runs).
pub fn package(
    repository: &Path,
    target_dir: &Path,
    packages: &[&Package],
    verify: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut args = vec!["package", "--locked"];
    if !verify {
        args.push("--no-verify");
    }
    for package in packages {
        args.extend(["--package", &package.name]);
    }
    let status = Command::new("cargo")
        .args(&args)
        .current_dir(repository)
        .status()
        .context("failed to run `cargo package`")?;
    if !status.success() {
        bail!("`cargo {}` failed", args.join(" "));
    }
    Ok(packages
        .iter()
        .map(|package| {
            target_dir
                .join("package")
                .join(crate_file(&package.name, &package.version))
        })
        .collect())
}

/// The file name of a packaged crate.
pub fn crate_file(name: &str, version: &str) -> String {
    format!("{name}-{version}.crate")
}

/// The SHA-256 of a file, hex-encoded.
pub fn sha256(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open `{}`", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Publishes packages to crates.io, in dependency order, without rebuilding
/// them. With `dry_run`, cargo checks everything but doesn't upload.
pub fn publish(repository: &Path, names: &[&str], dry_run: bool) -> anyhow::Result<()> {
    let mut args = vec!["publish", "--locked", "--no-verify"];
    if dry_run {
        args.push("--dry-run");
    }
    for name in names {
        args.extend(["--package", name]);
    }
    let status = Command::new("cargo")
        .args(&args)
        .current_dir(repository)
        .status()
        .context("failed to run `cargo publish`")?;
    if !status.success() {
        bail!("`cargo {}` failed", args.join(" "));
    }
    Ok(())
}

/// A GitHub release.
#[derive(Clone, Debug, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub draft: bool,
}

/// The repository's GitHub releases, drafts included.
pub fn github_releases(repository: &Path) -> anyhow::Result<Vec<GithubRelease>> {
    let output = run(
        repository,
        "gh",
        &[
            "api",
            "--paginate",
            "--slurp",
            "repos/{owner}/{repo}/releases?per_page=100",
        ],
    )?;
    let pages: Vec<Vec<GithubRelease>> =
        serde_json::from_str(&output).context("unexpected GitHub API response")?;
    Ok(pages.into_iter().flatten().collect())
}

/// Creates a draft release for `release`, or updates its title and notes if
/// it exists. Drafts don't create their tag.
pub fn upsert_draft(
    repository: &Path,
    release: &ManifestRelease,
    commit: &str,
    exists: bool,
) -> anyhow::Result<()> {
    let notes = tempfile::NamedTempFile::new()?;
    std::fs::write(notes.path(), &release.notes)?;
    let notes_path = notes.path().to_str().context("non-UTF-8 temporary path")?;
    let args: Vec<&str> = if exists {
        vec![
            "release",
            "edit",
            &release.tag,
            "--title",
            &release.title,
            "--notes-file",
            notes_path,
            "--target",
            commit,
        ]
    } else {
        vec![
            "release",
            "create",
            &release.tag,
            "--draft",
            "--title",
            &release.title,
            "--notes-file",
            notes_path,
            "--target",
            commit,
        ]
    };
    run(repository, "gh", &args)?;
    Ok(())
}

/// Uploads `files` to the (draft) release of `tag`, replacing existing assets.
pub fn upload_assets(repository: &Path, tag: &str, files: &[PathBuf]) -> anyhow::Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let mut args = vec![
        "release".to_owned(),
        "upload".to_owned(),
        tag.to_owned(),
        "--clobber".to_owned(),
    ];
    for file in files {
        args.push(file.to_str().context("non-UTF-8 asset path")?.to_owned());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(repository, "gh", &args)?;
    Ok(())
}

/// Creates the lightweight tag `tag` on `commit`. Succeeds if it already
/// points at `commit`, fails if it points elsewhere.
pub fn create_tag(repository: &Path, tag: &str, commit: &str) -> anyhow::Result<()> {
    let existing = Command::new("gh")
        .args([
            "api",
            &format!("repos/{{owner}}/{{repo}}/git/ref/tags/{tag}"),
            "--jq",
            ".object.sha",
        ])
        .current_dir(repository)
        .stderr(Stdio::null())
        .output()
        .context("failed to run `gh`")?;
    if existing.status.success() {
        let sha = String::from_utf8_lossy(&existing.stdout).trim().to_owned();
        if sha == commit {
            return Ok(());
        }
        bail!("`{tag}` already exists and points at {sha}, not {commit}");
    }
    run(
        repository,
        "gh",
        &[
            "api",
            "repos/{owner}/{repo}/git/refs",
            "-f",
            &format!("ref=refs/tags/{tag}"),
            "-f",
            &format!("sha={commit}"),
        ],
    )?;
    Ok(())
}

/// Publishes the draft release of `tag`, whose tag must exist.
pub fn publish_release(repository: &Path, tag: &str) -> anyhow::Result<()> {
    run(
        repository,
        "gh",
        &["release", "edit", tag, "--draft=false", "--verify-tag"],
    )?;
    Ok(())
}
