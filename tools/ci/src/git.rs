//! The Git operations the selector needs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

/// Runs `git` with `args` in `repository`, returning its stdout.
fn git(repository: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git produced non-UTF-8 output")
}

/// The root of the repository containing `dir`.
pub fn repository_root(dir: &Path) -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(
        git(dir, &["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

/// The best common ancestor of two revisions.
pub fn merge_base(repository: &Path, a: &str, b: &str) -> anyhow::Result<String> {
    Ok(git(repository, &["merge-base", a, b])?.trim().to_owned())
}

/// The paths that differ between `from` and `to`, relative to the repository
/// root. Renames are reported as a deletion plus an addition.
///
/// If `to` is `None`, compares against the working tree, including staged,
/// unstaged and untracked (but not ignored) files.
pub fn changed_paths(
    repository: &Path,
    from: &str,
    to: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut args = vec!["diff", "--name-only", "--no-renames", "-z", from];
    args.extend(to);
    let mut paths: Vec<String> = git(repository, &args)?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect();
    if to.is_none() {
        let untracked = git(
            repository,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?;
        paths.extend(
            untracked
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_owned),
        );
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// A detached, temporary checkout of a revision, removed on drop.
pub struct Worktree {
    repository: PathBuf,
    // Kept alive so that the directory outlives the worktree.
    dir: tempfile::TempDir,
}

impl Worktree {
    /// Checks out `revision` in a new temporary worktree.
    pub fn new(repository: &Path, revision: &str) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("failed to create a temporary directory")?;
        let path = dir
            .path()
            .to_str()
            .context("temporary directory path is not UTF-8")?;
        git(
            repository,
            &["worktree", "add", "--detach", "--force", path, revision],
        )?;
        Ok(Self {
            repository: repository.to_owned(),
            dir,
        })
    }

    /// The root of the checkout.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if let Some(path) = self.dir.path().to_str() {
            // Best effort: a leftover worktree entry is pruned by `git worktree prune`.
            let _ = git(&self.repository, &["worktree", "remove", "--force", path]);
        }
    }
}
