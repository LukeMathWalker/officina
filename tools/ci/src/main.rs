use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand};
use officina_ci::config::Platform;
use officina_ci::release::steps;
use officina_ci::{Selection, Snapshot, check, git, plan, select, snapshot};

/// Selects and runs the CI checks affected by a change.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Select the packages affected by a change and plan the CI jobs.
    ///
    /// Prints the plan on stdout, and why each package was selected on stderr.
    Select {
        #[command(flatten)]
        change: Change,
        /// The revision containing the change. Defaults to the working tree,
        /// including uncommitted and untracked files.
        #[arg(long)]
        head: Option<String>,
        /// Select every package, regardless of what changed.
        #[arg(long)]
        everything: bool,
        /// Append the plan to this file using the GitHub Actions output
        /// format (usually `$GITHUB_OUTPUT`).
        #[arg(long)]
        github_output: Option<PathBuf>,
    },
    /// Run the CI checks of packages on this platform.
    Check {
        /// Packages or projects to check. Defaults to the packages affected
        /// by the changes in the working tree.
        targets: Vec<String>,
        #[command(flatten)]
        change: Change,
        /// The platform to run the checks for. Defaults to the current one.
        #[arg(long)]
        platform: Option<Platform>,
    },
    /// Release the crates whose version isn't released yet, step by step.
    #[command(subcommand)]
    Release(ReleaseStep),
}

#[derive(Subcommand)]
enum ReleaseStep {
    /// Show what would be released, and why other crates aren't.
    Plan {
        /// Append the plan to this file using the GitHub Actions output format
        /// (usually `$GITHUB_OUTPUT`): whether there's anything to release,
        /// cargo-dist's build matrix, and the crates shipping binaries.
        #[arg(long)]
        github_output: Option<PathBuf>,
    },
    /// Package the crates to release (verified), extract their release notes,
    /// and write everything, with the release manifest, to a directory.
    Package {
        #[arg(long, default_value = "target/release")]
        out: PathBuf,
    },
    /// Create or update the draft GitHub releases, and upload their assets.
    Stage {
        /// The manifest written by `package`.
        #[arg(long)]
        manifest: PathBuf,
        /// The cargo-dist artifacts of each crate shipping binaries, in a
        /// directory named after the crate.
        #[arg(long)]
        assets: PathBuf,
        /// Only check and report what would be done.
        #[arg(long)]
        dry_run: bool,
    },
    /// Publish the packaged crates to crates.io.
    Publish {
        /// The manifest written by `package`.
        #[arg(long)]
        manifest: PathBuf,
        /// Check everything (`cargo publish --dry-run`), without uploading.
        #[arg(long)]
        dry_run: bool,
    },
    /// Create the tags, then publish the draft GitHub releases.
    Tag {
        /// The manifest written by `package`.
        #[arg(long)]
        manifest: PathBuf,
        /// Only report what would be done.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
struct Change {
    /// The revision the change is based on. Changes are computed from its
    /// merge base with the head.
    #[arg(long, default_value = "origin/main")]
    base: String,
}

fn main() -> anyhow::Result<()> {
    let repository = git::repository_root(&std::env::current_dir()?)?;
    match Cli::parse().command {
        Command::Select {
            change,
            head,
            everything,
            github_output,
        } => {
            let everything = everything.then_some("requested with `--everything`");
            let (selection, head) =
                select_changes(&repository, &change.base, head.as_deref(), everything)?;
            let jobs = plan::plan(&selection, &head);
            print_selection(&selection);
            let matrix = plan::to_matrix(&jobs);
            println!("{}", serde_json::to_string_pretty(&matrix)?);
            if let Some(path) = github_output {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("failed to open `{}`", path.display()))?;
                writeln!(file, "matrix={matrix}")?;
                writeln!(file, "projects={}", serde_json::json!(selection.projects))?;
            }
            if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
                write_step_summary(Path::new(&path), &selection, &jobs)?;
            }
        }
        Command::Release(step) => release(&repository, step)?,
        Command::Check {
            targets,
            change,
            platform,
        } => {
            let platform = match platform {
                Some(platform) => platform,
                None => Platform::current().context("unsupported platform: pass `--platform`")?,
            };
            let head = load_snapshot(&repository).context("failed to load the workspace")?;
            let packages = if targets.is_empty() {
                let (selection, _) = select_changes(&repository, &change.base, None, None)?;
                print_selection(&selection);
                selection.packages.into_keys().collect()
            } else {
                resolve_targets(&head, &targets)?
            };
            let steps = check::steps(&head, &packages, platform);
            if steps.is_empty() {
                eprintln!("Nothing to check on {platform}.");
                return Ok(());
            }
            let failures = check::run(&steps, &repository);
            eprintln!();
            if failures.is_empty() {
                eprintln!("All {} checks passed.", steps.len());
            } else {
                eprintln!("{} of {} checks failed:", failures.len(), steps.len());
                for step in &failures {
                    eprintln!("  {}: {}\n    $ {step}", step.package, step.name);
                }
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// Selects the packages affected by the changes from the merge base of
/// `base` and `head` (or the working tree) to `head`.
fn select_changes(
    repository: &Path,
    base: &str,
    head: Option<&str>,
    everything: Option<&str>,
) -> anyhow::Result<(Selection, Snapshot)> {
    let merge_base = git::merge_base(repository, base, head.unwrap_or("HEAD"))?;
    let changed_paths = git::changed_paths(repository, &merge_base, head)?;
    let head_snapshot = match head {
        Some(revision) => load_snapshot(git::Worktree::new(repository, revision)?.path()),
        None => load_snapshot(repository),
    }
    .context("failed to load the workspace at the head revision")?;
    let base_snapshot = git::Worktree::new(repository, &merge_base)
        .and_then(|worktree| load_snapshot(worktree.path()))
        .inspect_err(|error| {
            eprintln!("warning: failed to load the workspace at the base revision: {error:#}")
        })
        .ok();
    eprintln!(
        "{} changed path(s) since {}",
        changed_paths.len(),
        &merge_base[..12.min(merge_base.len())]
    );
    let selection = select(
        base_snapshot.as_ref(),
        &head_snapshot,
        &changed_paths,
        everything,
    );
    Ok((selection, head_snapshot))
}

/// Loads the workspace rooted at `dir`, requiring an up-to-date `Cargo.lock`.
fn load_snapshot(dir: &Path) -> anyhow::Result<Snapshot> {
    snapshot(&load_graph(dir)?)
}

/// Loads the package graph of the workspace rooted at `dir`, requiring an
/// up-to-date `Cargo.lock`.
fn load_graph(dir: &Path) -> anyhow::Result<guppy::graph::PackageGraph> {
    guppy::MetadataCommand::new()
        .current_dir(dir)
        .other_options(["--locked".to_owned()])
        .build_graph()
        .with_context(|| format!("`cargo metadata --locked` failed in `{}`", dir.display()))
}

fn release(repository: &Path, step: ReleaseStep) -> anyhow::Result<()> {
    // `stage` and `tag` only need the manifest and GitHub: no Rust toolchain.
    match step {
        ReleaseStep::Plan { github_output } => {
            let (plan, _) = steps::plan(repository, &load_graph(repository)?)?;
            steps::print_plan(&plan);
            if let Some(path) = github_output {
                let dist_matrix = steps::dist_matrix(repository, &plan)?;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("failed to open `{}`", path.display()))?;
                writeln!(file, "has-release={}", !plan.crates.is_empty())?;
                writeln!(file, "dist-matrix={dist_matrix}")?;
                let dist_releases: Vec<_> = plan
                    .releases
                    .iter()
                    .filter_map(|release| {
                        let krate = release.dist.as_ref()?;
                        Some(serde_json::json!({ "crate": krate, "tag": release.tag }))
                    })
                    .collect();
                writeln!(
                    file,
                    "dist-releases={}",
                    serde_json::json!({ "include": dist_releases })
                )?;
            }
        }
        ReleaseStep::Package { out } => {
            steps::package(repository, &load_graph(repository)?, &repository.join(out))?;
        }
        ReleaseStep::Stage {
            manifest,
            assets,
            dry_run,
        } => {
            steps::stage(
                repository,
                &steps::read_manifest(&manifest)?,
                &assets,
                dry_run,
            )?;
        }
        ReleaseStep::Publish { manifest, dry_run } => {
            let manifest = steps::read_manifest(&manifest)?;
            steps::publish(repository, &load_graph(repository)?, &manifest, dry_run)?;
        }
        ReleaseStep::Tag { manifest, dry_run } => {
            steps::tag(repository, &steps::read_manifest(&manifest)?, dry_run)?;
        }
    }
    Ok(())
}

/// Expands package and project names into package names.
fn resolve_targets(head: &Snapshot, targets: &[String]) -> anyhow::Result<Vec<String>> {
    let mut packages = Vec::new();
    for target in targets {
        if head.members.contains_key(target) {
            packages.push(target.clone());
            continue;
        }
        let project: Vec<String> = head
            .members
            .iter()
            .filter(|(_, member)| member.project() == target)
            .map(|(name, _)| name.clone())
            .collect();
        if project.is_empty() {
            bail!("`{target}` is neither a workspace package nor a project");
        }
        packages.extend(project);
    }
    packages.sort();
    packages.dedup();
    Ok(packages)
}

fn print_selection(selection: &Selection) {
    match &selection.everything {
        Some(why) => eprintln!("Selected all {} packages: {why}", selection.packages.len()),
        None if selection.packages.is_empty() => eprintln!("Selected no packages"),
        None => {
            eprintln!("Selected {} package(s):", selection.packages.len());
            for (name, reason) in &selection.packages {
                eprintln!("  {name}: {reason}");
            }
        }
    }
}

/// Writes the selection and plan as Markdown, for the workflow run's summary page.
fn write_step_summary(
    path: &Path,
    selection: &Selection,
    jobs: &[plan::Job],
) -> anyhow::Result<()> {
    let mut out = String::from("## CI plan\n\n");
    match &selection.everything {
        Some(why) => out.push_str(&format!("Checking every package: {why}.\n\n")),
        None if jobs.is_empty() => out.push_str("No package is affected by this change.\n\n"),
        None => {
            out.push_str("| Package | Selected because |\n|---|---|\n");
            for (name, reason) in &selection.packages {
                out.push_str(&format!("| `{name}` | {reason} |\n"));
            }
            out.push('\n');
        }
    }
    if !jobs.is_empty() {
        out.push_str("| Job | Packages |\n|---|---|\n");
        for job in jobs {
            out.push_str(&format!(
                "| {} | {} |\n",
                job.name(),
                job.packages.join(", ")
            ));
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(out.as_bytes())?;
    Ok(())
}
