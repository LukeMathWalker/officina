//! Runs the CI checks of packages, one package at a time.
//!
//! Checking packages one at a time, rather than with a single cargo
//! invocation, keeps Cargo from unifying features across them: each package
//! must build with the features it declares.
//!
//! For each package, on each platform it supports:
//!
//! - `cargo clippy` (warnings denied) and, unless `tests = false`,
//!   `cargo nextest run`, for each feature set;
//!
//! and, on Linux only:
//!
//! - `cargo fmt --check`;
//! - doctests and `cargo doc` (warnings denied, library targets only), with the
//!   docs feature set;
//! - the package's custom commands.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::Snapshot;
use crate::config::{FeatureSet, Platform};

/// A command to run as part of a package's checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub package: String,
    /// What the step checks, e.g. `clippy (all)`.
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    /// The working directory, relative to the repository root.
    pub dir: String,
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.dir.is_empty() {
            write!(f, "(in {}) ", self.dir)?;
        }
        for (key, value) in &self.env {
            write!(f, "{key}={value:?} ")?;
        }
        write!(f, "{} {}", self.program, self.args.join(" "))
    }
}

/// The steps that check `packages` on `platform`. Packages that don't
/// support `platform` are skipped.
pub fn steps(head: &Snapshot, packages: &[String], platform: Platform) -> Vec<Step> {
    let mut steps = Vec::new();
    for package in packages {
        let member = &head.members[package];
        let ci = &member.ci;
        if !ci.platforms.contains(&platform) {
            continue;
        }
        let step =
            |name: String, program: &str, args: Vec<String>, env: BTreeMap<String, String>| Step {
                package: package.clone(),
                name,
                program: program.to_owned(),
                args,
                dir: String::new(),
                env,
            };
        let cargo = |subcommand: &[&str], set: &FeatureSet, extra: &[&str]| -> Vec<String> {
            let mut args: Vec<String> = subcommand.iter().map(|arg| arg.to_string()).collect();
            args.extend(["--locked".into(), "--package".into(), package.clone()]);
            args.extend(set.args.iter().cloned());
            args.extend(extra.iter().map(|arg| arg.to_string()));
            args
        };

        if platform == Platform::Linux {
            steps.push(step(
                "fmt".into(),
                "cargo",
                vec![
                    "fmt".into(),
                    "--check".into(),
                    "--package".into(),
                    package.clone(),
                ],
                BTreeMap::new(),
            ));
        }
        for set in &ci.feature_sets {
            let env = rustflags_env(set.rustflags.as_deref());
            steps.push(step(
                format!("clippy ({})", set.name),
                "cargo",
                cargo(&["clippy"], set, &["--all-targets", "--", "-D", "warnings"]),
                env.clone(),
            ));
            if ci.tests {
                steps.push(step(
                    format!("tests ({})", set.name),
                    "cargo",
                    cargo(&["nextest", "run"], set, &["--no-tests=pass"]),
                    env,
                ));
            }
        }
        if platform == Platform::Linux && member.has_lib {
            let set = ci.docs_feature_set();
            let mut env = rustflags_env(set.rustflags.as_deref());
            if let Some(rustflags) = &set.rustflags {
                env.insert("RUSTDOCFLAGS".into(), rustflags.clone());
            }
            steps.push(step(
                format!("doctests ({})", set.name),
                "cargo",
                cargo(&["test"], set, &["--doc"]),
                env.clone(),
            ));
            let rustdocflags = match &set.rustflags {
                Some(rustflags) => format!("-D warnings {rustflags}"),
                None => "-D warnings".into(),
            };
            env.insert("RUSTDOCFLAGS".into(), rustdocflags);
            steps.push(step(
                format!("docs ({})", set.name),
                "cargo",
                cargo(&["doc"], set, &["--lib", "--no-deps"]),
                env,
            ));
        }
        if platform == Platform::Linux {
            for command in &ci.commands {
                steps.push(Step {
                    dir: member.root.clone(),
                    ..step(
                        command.name.clone(),
                        "bash",
                        vec!["-c".into(), command.run.clone()],
                        rustflags_env(command.rustflags.as_deref()),
                    )
                });
            }
        }
    }
    steps
}

/// The environment for commands built with custom `RUSTFLAGS`.
///
/// They get their own target directory: changing `RUSTFLAGS` invalidates
/// every artifact, so sharing one would rebuild everything back and forth.
fn rustflags_env(rustflags: Option<&str>) -> BTreeMap<String, String> {
    let Some(rustflags) = rustflags else {
        return BTreeMap::new();
    };
    let slug: String = rustflags
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    BTreeMap::from([
        ("RUSTFLAGS".into(), rustflags.to_owned()),
        ("CARGO_TARGET_DIR".into(), format!("target/ci/{slug}")),
    ])
}

/// Runs `steps` from `repository`, returning the ones that failed.
///
/// On GitHub Actions, each step's output is folded into a log group and
/// failures are reported as annotations.
pub fn run<'a>(steps: &'a [Step], repository: &Path) -> Vec<&'a Step> {
    let github = std::env::var_os("GITHUB_ACTIONS").is_some_and(|value| value == "true");
    let mut failures = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let title = format!(
            "[{}/{}] {}: {}",
            i + 1,
            steps.len(),
            step.package,
            step.name
        );
        if github {
            println!("::group::{title}");
        } else {
            eprintln!("\n=== {title}");
        }
        eprintln!("$ {step}");
        let mut command = Command::new(&step.program);
        command
            .args(&step.args)
            .current_dir(repository.join(&step.dir));
        for (key, value) in &step.env {
            // Relative target directories are relative to the repository root, not the step's directory.
            let value = if key == "CARGO_TARGET_DIR" {
                repository.join(value).to_string_lossy().into_owned()
            } else {
                value.clone()
            };
            command.env(key, value);
        }
        let success = match command.status() {
            Ok(status) => status.success(),
            Err(error) => {
                eprintln!("failed to run `{}`: {error}", step.program);
                false
            }
        };
        if github {
            println!("::endgroup::");
        }
        if !success {
            if github {
                println!(
                    "::error title={}: {} failed::{step}",
                    step.package, step.name
                );
            }
            failures.push(step);
        }
    }
    failures
}
