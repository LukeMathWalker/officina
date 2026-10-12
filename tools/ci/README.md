# officina-ci

Selects and runs the CI checks affected by a change.

```bash
# Check the packages affected by your changes (vs. origin/main), like CI would.
cargo ci check
# Check specific packages or whole projects.
cargo ci check biscotti cheadergen
# Show what CI would run for your changes, and why.
cargo ci select
# Show what would be released, and why other crates aren't.
cargo ci release plan
```

## How CI works

1. **Plan.** `cargo ci select` computes the packages a change affects: the
   packages owning the changed files, their dependents in the workspace, and the
   dependents of third-party packages whose version changed in `Cargo.lock`.
   Repository-wide inputs (root `Cargo.toml`, toolchain, `.cargo/`, `.config/`,
   CI workflows, this tool) and pushes to `main` select everything. It then plans
   one job per project (top-level directory) and platform.
2. **Check.** Each job runs `cargo ci check` for its packages, one package at a
   time (so that Cargo doesn't unify features across packages):
   - clippy (warnings denied) and nextest, for each feature set;
   - on Linux only: `cargo fmt --check`, doctests and docs (warnings denied),
     and the package's custom commands.
3. **Project workflows.** A project needing more than that has its own reusable
   workflow, `.github/workflows/<project>.yml`, called by `ci.yml` when the
   project is affected (e.g. `cheadergen.yml`).

`CI result` aggregates every job: it is the check to require on `main`.

## Configuring a package

Packages get the defaults unless their manifest says otherwise:

```toml
[package.metadata.ci]
# Platforms to check the package on. Default: `["linux"]`.
platforms = ["linux", "macos", "windows"]
# Whether to run the package's tests with nextest. Default: `true`.
tests = true
# Feature combinations to lint and test. Default: `default`, `all`
# (`--all-features`, used for docs), and `no-default`.
feature-sets = [
    { name = "default", docs = true },
    { name = "unstable", args = ["--features=unstable"] },
    # `rustflags` apply to every command of the set.
    { name = "cfg", rustflags = "--cfg unstable" },
]
# Extra commands, run with `bash` in the package directory, on Linux only.
commands = [{ name = "README is up to date", run = "cargo rdme --check" }]
# Tools the commands need, installed in CI with `taiki-e/install-action`.
tools = ["cargo-rdme"]
```

Unknown fields are rejected, so typos fail CI instead of being ignored.

## Releases

`cargo ci release <step>` releases the crates whose version isn't released yet.
Versions and changelogs are bumped beforehand, in release PRs (`release-plz.toml`).

1. `plan`: the crates to release, their tags (`<crate>-v<version>`) and their
   GitHub releases.
2. `package`: verified `.crate` files, release notes from the changelogs, and a
   manifest with their checksums. cargo-dist builds the binaries
   (`dist-workspace.toml`, and `[package.metadata.dist] dist = true`).
3. `stage`: draft GitHub releases, with the binaries attached.
4. `publish`: repackage, compare with the manifest's checksums, then upload to
   crates.io.
5. `tag`: create the tags, then publish the draft releases.

**A tag marks a completed release.** A crate is released if its version has no
tag, and isn't on crates.io (or is, with our checksum: a previous run published
it but didn't finish). Every step is idempotent: rerunning an interrupted
release finishes it. `stage`, `publish` and `tag` take `--dry-run`.

In CI:

- `.github/workflows/release-pr.yml` opens or updates the release PR on every
  push to `main` (release-plz, with a GitHub App's token so that CI runs on it:
  the `RELEASE_PLZ_APP_CLIENT_ID` variable and `RELEASE_PLZ_APP_PRIVATE_KEY` secret).
- `.github/workflows/release.yml` runs after CI passes on `main`: plan, package
  and build binaries (read-only), stage, publish (trusted publishing, in the
  `release` environment), then tag. Run it manually for a dry run, on any branch
  (e.g. a release PR's), or for real on `main` to finish an interrupted release.
- CI runs `cargo ci release plan` on every PR, so release problems show up
  before merging.

By default, each crate gets its own GitHub release. Crates versioned together
can share one:

```toml
[package.metadata.release]
# The GitHub release's name. Default: the crate's name.
name = "cheadergen"
# The changelog with the release notes, relative to the package directory.
changelog = "../CHANGELOG.md"
# Crates released together with this one, in this crate's GitHub release.
includes = ["cheadergen", "cheadergen_macros"]
```

A crate shipping binaries must have its own release (or include others):
its installers download from its tag.
