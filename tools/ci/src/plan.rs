//! Turns a [`Selection`] into CI jobs: one per project and platform.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::Platform;
use crate::{Selection, Snapshot};

/// A CI job: check some packages of a project on a platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub project: String,
    pub platform: Platform,
    /// The selected packages of the project that run on `platform`.
    pub packages: Vec<String>,
    /// The tools the checks need, in `taiki-e/install-action` syntax.
    pub tools: Vec<String>,
}

impl Job {
    /// The job's display name.
    pub fn name(&self) -> String {
        match self.platform {
            Platform::Linux => self.project.clone(),
            platform => format!("{} ({platform})", self.project),
        }
    }

    /// The job as an entry of a GitHub Actions matrix.
    pub fn to_matrix_entry(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name(),
            "project": self.project,
            "platform": self.platform.as_str(),
            "runner": self.platform.runner(),
            "packages": self.packages.join(" "),
            "tools": self.tools.join(","),
        })
    }
}

/// The jobs for `selection`, ordered by project, then platform.
pub fn plan(selection: &Selection, head: &Snapshot) -> Vec<Job> {
    let mut jobs: BTreeMap<(String, Platform), Job> = BTreeMap::new();
    for name in selection.packages.keys() {
        let member = &head.members[name];
        for &platform in &member.ci.platforms {
            let job = jobs
                .entry((member.project().to_owned(), platform))
                .or_insert_with(|| Job {
                    project: member.project().to_owned(),
                    platform,
                    packages: Vec::new(),
                    tools: Vec::new(),
                });
            job.packages.push(name.clone());
        }
    }
    for job in jobs.values_mut() {
        let mut tools = BTreeSet::new();
        for name in &job.packages {
            let ci = &head.members[name].ci;
            if ci.tests {
                tools.insert("nextest".to_owned());
            }
            // Custom commands only run on Linux.
            if job.platform == Platform::Linux {
                tools.extend(ci.tools.iter().cloned());
            }
        }
        job.tools = tools.into_iter().collect();
    }
    jobs.into_values().collect()
}

/// The jobs as a GitHub Actions matrix (`{"include": [...]}`).
pub fn to_matrix(jobs: &[Job]) -> serde_json::Value {
    serde_json::json!({ "include": jobs.iter().map(Job::to_matrix_entry).collect::<Vec<_>>() })
}
