//! Selects and runs the CI checks affected by a change.
//!
//! - [`select`](mod@select) computes the workspace packages a change affects.
//! - [`plan`](mod@plan) turns them into CI jobs, one per project and platform.
//! - [`check`](mod@check) runs a job's checks, package by package.
//! - [`config`] reads each package's `[package.metadata.ci]` configuration.
//! - [`release`] packages, stages, publishes and tags releases.

pub mod check;
pub mod config;
pub mod git;
pub mod plan;
pub mod release;
pub mod select;

pub use select::{ExternalPackage, Member, Reason, Selection, Snapshot, select, snapshot};
