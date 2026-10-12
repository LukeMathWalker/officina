# officina-ci

Selects and runs the CI checks affected by a change.

```bash
# Check the packages affected by your changes (vs. origin/main), like CI would.
cargo ci check
# Check specific packages or whole projects.
cargo ci check biscotti cheadergen
# Show what CI would run for your changes, and why.
cargo ci select
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
