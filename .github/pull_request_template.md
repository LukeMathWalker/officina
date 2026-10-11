<!--
Keep this description skimmable, and delete any section that does not apply.

Title: `type(scope)!: description`
- Types: build, chore, ci, docs, feat, fix, perf, refactor, revert, style, test
- Scope: the project directory the change is about (e.g. `biscotti`,
  `cheadergen`), or `workspace` for repository-wide changes.
- `!` marks a breaking change to a published crate.
-->

## Why

<!--
1-3 sentences. What problem does this solve, and why now?
Link the issue or discussion instead of repeating background.
-->

## What changed

<!-- 1-5 short bullets. Meaningful changes, not an implementation play-by-play. -->

-

## Public API / semver

<!--
Delete this section if no published crate's public API changes.

Otherwise, for each affected crate: what changed for downstream users, and
whether it is additive, breaking, or bumps the MSRV.
-->

## Extra verification

<!--
Delete this section unless you validated something beyond CI.

Don't list `cargo test`, clippy, rustfmt, or CI jobs here. Use it for manual
testing, platform-specific checks, or gaps automated tests can't cover.
Include exact steps, logs, or environment details.
-->

## Notes for reviewers

<!--
Delete this section if there is nothing notable.

Things a reviewer might miss: tricky logic or trade-offs, invariants to
double-check, follow-up work, known limitations.
-->
