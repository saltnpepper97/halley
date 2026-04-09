# Contributing to Halley

First: thank you for taking an interest in Halley.

Halley is a personal, vision-driven project. Contributions are welcome, but they must fit the direction of the compositor as defined by the maintainer. Please read this document before opening issues or pull requests.

## Project Direction

Halley is not a democracy.

This project has a specific design and interaction vision. Suggestions, patches, and discussion are welcome, but final decisions about architecture, behavior, naming, UX, scope, and release priorities belong to the maintainer.

Contributing does **not** guarantee influence over the roadmap.

If a proposal conflicts with the project’s goals, style, or long-term design, it may be declined even if it is technically sound.

## What Contributions Are Most Helpful

The most useful contributions are:

- focused bug fixes
- correctness fixes
- test improvements
- performance improvements that do not distort design
- protocol support that fits existing direction
- code cleanup with clear practical value
- documentation improvements
- narrowly scoped polish work approved in advance

These are usually much easier to review and merge than broad behavioral or architectural changes.

## Contributions That Need Discussion First

Please open an issue or discussion before spending major time on any of the following:

- architectural rewrites
- interaction model changes
- animation model changes
- window management policy changes
- clustering/workspace behavior changes
- docking behavior changes
- rendering pipeline changes
- config format changes
- IPC or CLI surface changes
- large refactors
- anything that adds substantial maintenance burden

Unapproved large pull requests may be closed without detailed review.

## What Will Likely Be Rejected

To save everyone time, the following are poor fits unless explicitly requested:

- changes that dilute or replace Halley’s core UX vision
- changes that add many knobs for niche preferences
- speculative abstractions without current need
- broad “cleanup” PRs that mix unrelated edits
- style-only PRs with no practical improvement
- dependency additions without strong justification
- changes that make the code harder to reason about
- “common compositor behavior” arguments without regard for Halley’s goals
- repeated attempts to relitigate already settled design decisions

“Other compositors do it this way” is not, by itself, a strong reason for Halley to do the same.

## Before You Open an Issue

Please make a good-faith effort to check:

- whether the behavior is intentional
- whether the topic has already been discussed
- whether the report is actually reproducible
- whether the problem belongs to Halley rather than an app/toolkit/protocol quirk

Good issues are specific, calm, and reproducible.

Please include:

- a clear description of the problem
- steps to reproduce
- expected behavior
- actual behavior
- logs, screenshots, or video if useful
- environment details relevant to the issue

Issues that are vague, hostile, demanding, or purely rhetorical may be closed.

## Feature Requests

Feature requests are allowed, but they should be framed as proposals, not demands.

A good feature request explains:

- the problem being solved
- why it fits Halley specifically
- how it aligns with the project’s design
- tradeoffs or maintenance cost
- whether it can be implemented incrementally

Requests that amount to “make Halley behave like another compositor/window manager” are unlikely to be accepted unless they clearly fit Halley’s direction.

## Pull Request Rules

### Keep PRs Small

Small, focused pull requests are much more likely to be reviewed.

A PR should ideally do **one thing**. If it fixes a bug, do not also refactor half the surrounding module. If it refactors something, do not also change behavior unless that behavior change is the point of the PR.

### Explain the Why

Every PR should explain:

- what changed
- why it changed
- whether behavior changed
- what risks or tradeoffs exist
- how it was tested

### Tests and Verification

If practical, include tests.

At minimum, contributors should run relevant checks before opening a PR. For example:

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
