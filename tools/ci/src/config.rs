//! Per-package CI configuration, read from `[package.metadata.ci]`.
//!
//! Every field is optional: packages without the table get the defaults.
//!
//! ```toml
//! [package.metadata.ci]
//! # Platforms to check the package on. Default: `["linux"]`.
//! platforms = ["linux", "macos", "windows"]
//! # Whether to run the package's tests with nextest. Default: `true`.
//! # Set to `false` when they run in a dedicated workflow.
//! tests = true
//! # The feature combinations to lint and test. Default: `default`, `all`
//! # (`--all-features`, also used for docs), and `no-default`.
//! feature-sets = [
//!     { name = "default" },
//!     # `rustflags` are set for every command of the set. `docs` marks the
//!     # set used for docs and doctests (at most one; default: the first).
//!     { name = "unstable", args = ["--features", "unstable"], rustflags = "--cfg unstable", docs = true },
//! ]
//! # Extra commands, run with `bash` in the package directory, on Linux only.
//! commands = [{ name = "README is up to date", run = "cargo rdme --check" }]
//! # Tools the commands need, installed in CI by `taiki-e/install-action`.
//! tools = ["cargo-rdme"]
//! ```

use std::collections::BTreeSet;

use serde::Deserialize;

/// A platform CI can check packages on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Macos,
    Windows,
}

impl Platform {
    /// The platform this binary was built for, if CI supports it.
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "linux" => Some(Self::Linux),
            "macos" => Some(Self::Macos),
            "windows" => Some(Self::Windows),
            _ => None,
        }
    }

    /// The GitHub Actions runner image for this platform.
    pub fn runner(self) -> &'static str {
        match self {
            Self::Linux => "ubuntu-latest",
            Self::Macos => "macos-latest",
            Self::Windows => "windows-latest",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Platform {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "linux" => Ok(Self::Linux),
            "macos" => Ok(Self::Macos),
            "windows" => Ok(Self::Windows),
            _ => Err(format!(
                "unknown platform `{s}`: expected linux, macos or windows"
            )),
        }
    }
}

/// The CI configuration of a package.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CiConfig {
    #[serde(default = "default_platforms")]
    pub platforms: Vec<Platform>,
    #[serde(default = "default_tests")]
    pub tests: bool,
    #[serde(default = "default_feature_sets")]
    pub feature_sets: Vec<FeatureSet>,
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
    #[serde(default)]
    pub tools: Vec<String>,
}

/// A feature combination to lint and test.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureSet {
    pub name: String,
    /// Extra arguments for cargo, e.g. `["--all-features"]`.
    #[serde(default)]
    pub args: Vec<String>,
    /// `RUSTFLAGS` for every command of this set.
    #[serde(default)]
    pub rustflags: Option<String>,
    /// Whether docs and doctests are built with this set.
    #[serde(default)]
    pub docs: bool,
}

/// An extra command, run with `bash` in the package directory.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomCommand {
    pub name: String,
    pub run: String,
    /// `RUSTFLAGS` for the command.
    #[serde(default)]
    pub rustflags: Option<String>,
}

fn default_platforms() -> Vec<Platform> {
    vec![Platform::Linux]
}

fn default_tests() -> bool {
    true
}

fn default_feature_sets() -> Vec<FeatureSet> {
    let set = |name: &str, args: &[&str], docs| FeatureSet {
        name: name.into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        rustflags: None,
        docs,
    };
    vec![
        set("default", &[], false),
        set("all", &["--all-features"], true),
        set("no-default", &["--no-default-features"], false),
    ]
}

impl Default for CiConfig {
    fn default() -> Self {
        Self {
            platforms: default_platforms(),
            tests: default_tests(),
            feature_sets: default_feature_sets(),
            commands: Vec::new(),
            tools: Vec::new(),
        }
    }
}

impl CiConfig {
    /// Reads the configuration from a package's `[package.metadata]` table.
    pub fn from_metadata(metadata: &serde_json::Value) -> anyhow::Result<Self> {
        let config = match metadata.get("ci") {
            None => Self::default(),
            Some(table) => Self::deserialize(table)?,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.platforms.is_empty(), "`platforms` must not be empty");
        anyhow::ensure!(
            !self.feature_sets.is_empty(),
            "`feature-sets` must not be empty"
        );
        let mut names = BTreeSet::new();
        for set in &self.feature_sets {
            anyhow::ensure!(
                names.insert(&set.name),
                "duplicate feature set `{}`",
                set.name
            );
        }
        anyhow::ensure!(
            self.feature_sets.iter().filter(|set| set.docs).count() <= 1,
            "at most one feature set can set `docs = true`"
        );
        Ok(())
    }

    /// The feature set used for docs and doctests.
    pub fn docs_feature_set(&self) -> &FeatureSet {
        self.feature_sets
            .iter()
            .find(|set| set.docs)
            .unwrap_or(&self.feature_sets[0])
    }
}
