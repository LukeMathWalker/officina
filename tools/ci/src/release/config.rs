//! Per-package release configuration, read from `[package.metadata.release]`.
//!
//! Every published crate gets a tag (`<crate>-v<version>`). By default, it also
//! gets its own GitHub release, named after the crate, with release notes taken
//! from `CHANGELOG.md` in the package directory. A crate can instead group other
//! crates, versioned together with it, into its GitHub release:
//!
//! ```toml
//! [package.metadata.release]
//! # The GitHub release's name. Default: the crate's name.
//! name = "cheadergen"
//! # The changelog with the release notes, relative to the package directory.
//! changelog = "../CHANGELOG.md"
//! # Crates released together with this one, in this crate's GitHub release.
//! includes = ["cheadergen", "cheadergen_macros"]
//! ```

use serde::Deserialize;

/// The release configuration of a package.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseConfig {
    pub name: Option<String>,
    pub changelog: Option<String>,
    #[serde(default)]
    pub includes: Vec<String>,
}

impl ReleaseConfig {
    /// Reads the configuration from a package's `[package.metadata]` table.
    pub fn from_metadata(metadata: &serde_json::Value) -> anyhow::Result<Self> {
        Ok(match metadata.get("release") {
            None => Self::default(),
            Some(table) => Self::deserialize(table)?,
        })
    }
}
