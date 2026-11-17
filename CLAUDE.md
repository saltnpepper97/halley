# Halley: notes for agents

Keep `CLAUDE.md` and `AGENTS.md` identical. When you change one, copy it over
the other in the same commit.

This file holds settled design and how we work, not progress notes.

## Settled design

Do not change any of these on your own. If a task seems to need one changed,
stop and tell the user. Once they agree, update this file and `AGENTS.md`.

Halley's design is being reworked (from November 2026): decay first, then
clusters, Arrange and docks. As each change lands, update this section in the
same branch so it always describes Halley as it is.

### Window behavior
- **Fullscreen and window switching:** When switching away from a fullscreen
  game to another window on the same monitor, pause its fullscreen state
  without exiting fullscreen; restore it when returning. Switching to a window
  on another monitor must leave the game fullscreen on its original monitor. A
  regular window made fullscreen with Mod+F, such as a browser playing
  fullscreen YouTube, should instead leave fullscreen when switching away to
  another window on the same monitor.
- **Decay:** Automatic decay is off by default. When enabled, it applies only
  to windows entirely off screen; a window with any part visible, at any zoom
  level, never decays. There is no decay zone: the screen edge is the only
  boundary.
- **Pinned nodes:** Pinning exempts a window from automatic decay. It must
  still be possible to manually convert it to a node and restore it from a
  node.
- **Empty clusters:** An empty cluster must show its close hint regardless of
  cursor position or monitor focus. The hint should explain that Super+Q,
  followed by Super+Q again, deletes the cluster. Preserve the existing cluster
  design and behavior unless the user requests a change.
- **Closing collapsed windows:** A window collapsed into a node must remain
  closable with Mod+Q when its node is focused, including when hover focus
  selects it.
- **XWayland:** Preserve proper EWMH compliance when implementing or changing
  XWayland behavior.

### Upstream dependencies
Smithay and smithay-drm-extras must use the same explicitly pinned revision of
the unmodified official `https://github.com/Smithay/smithay.git` repository.
Keep the revision in Cargo.toml and Cargo.lock consistent; update the pin
deliberately when needed. Never vendor or bundle Smithay in the repository or
release archives, copy its implementation into Halley, use a modified fork, or
apply dependency patches or repository source replacements. Implement Halley
policy in Halley through public upstream APIs. Do not weaken IME lock
isolation or recovery to accommodate a dependency change. The upstream-source
CI guard must pass before merging.

Do not place a cloned or modified Smithay source tree anywhere in the project.
Keep the current upstream pin unless there is a deliberate reason to update
it; Halley currently needs a revision newer than Smithay 0.7 for blur support.

## How we work

### Code
- Nothing user-facing is hard-coded. Any value a user could reasonably want
  different (colours, opacities, sizes, gaps, timings, keys) is a config
  setting, with its default defined once in `halley-config`. Every option is
  documented in `examples/halley.reference.rune` and in `docs/`. The starter
  (`halley-config/halley.default.rune`, mirrored by `examples/halley.rune`)
  holds only the settings people change first, which Dustin decides, so
  nothing goes into it without his say. Every new option also goes into
  Dustin's own config (`~/.config/halley/halley.rune`), edited in place, so he
  can test it. Code keeps only true internals (tolerances, cache sizes,
  protocol constants) and the settled design rules.
- Keep the structure solid as it grows: high cohesion, low coupling, high
  maintainability. Each module owns one concern, imports point one way, and a
  rule lives in exactly one place. `halley-core` is the model and never
  depends on Wayland or Smithay.
- When a setting is renamed or retired, existing configs keep loading: an old
  spelling keeps working, or a retired setting is accepted and ignored, and
  `docs/older-spellings.md` says which. Halley never rewrites a user's config.
- Compatibility to remove eventually, once the user agrees configs have moved
  on: `decay.outside-delay-seconds` as an older spelling of
  `decay.delay-seconds`, and accepting and ignoring the retired
  `decay.inside-delay-seconds`, `view.output` → `decay-zone:`/`focus-ring:`,
  and `debug.show-decay-zone`/`show-focus-ring`. Keep them until then so
  existing configs don't break.

### Testing
Run the local CI checks before merging: the Smithay source guard,
`cargo fmt --check`, clippy with `-D warnings` for all three feature sets,
workspace tests, and the docs build.

Do not run the nested compositor suite
(`HALLEY_TEST_WINIT_SERVER=… cargo test --test xwayland_disconnect -- --ignored`,
about four minutes) for every change. Run it only when a change affects
behaviour that needs a live compositor to observe: window, fullscreen,
maximize, focus, pointer or keyboard handling, XWayland, rendering, or
capture. Config, docs, CLI and pure-refactor changes rely on CI's headless
XWayland job instead. When a change does need a live compositor, run only the
nested tests that cover it
(`cargo test --test xwayland_disconnect <name> -- --ignored`); keep the full
suite for broad changes to window management, fullscreen, focus policy or
XWayland. A small overlay or notification change with its own targeted test
does not need the full suite. The nested windows open on the user's real
desktop, but test builds ignore host keyboard and pointer input, so tests are
driven only through their control socket and their own clients.

### Installing
- Build with `cargo build --release --workspace`; without `--workspace` only
  `halley` is rebuilt, not `halleyctl` or the portal.
- Install by copying `halley`, `halleyctl` and `xdg-desktop-portal-halley`
  over the old ones in `~/.local/bin`. Keep no backups of the binaries or of
  the user's `halley.rune`, anywhere: git is the backup, and any old build can
  be rebuilt from it.

### Commits
- Commit as you go. One commit is one piece of work you can describe in a
  sentence or two: what you did and what it solves, or why.
- Use conventional prefixes (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`,
  `chore:`, `perf:`). No AI attribution: no `Co-Authored-By` trailers and no
  "Generated with" lines.
- Never commit directly to `main`. Where a change goes depends on its size:
  - **Significant changes** (a feature, a fix with real behavior, a refactor,
    anything a reviewer should see as a PR) go on their own branch and reach
    `main` through a pull request.
  - **Small changes** (README or doc wording, agent notes, a typo, a one-line
    tweak) are committed on `dev` and merged straight into `main` with no
    branch and no PR. See the direct flow below.
  - If you can't tell which one a change is, ask.
- Before your first commit, check the current branch. For a significant change
  on `main` or `dev`, branch first. For a small change, switch to `dev`.
- Name each branch for the kind of change: `feat/`, `fix/`, `refactor/`,
  `docs/`, `test/`, `chore/`, `perf/` or `release/`, then a short kebab-case
  description (`feat/offscreen-only-decay`). Never use tool-named prefixes
  such as `claude/` or `codex/`.
- Keep only `main` and `dev` plus the one branch being worked on.
- Don't push until the user asks.

### Versioning
Use MAJOR.MINOR.PATCH, version components independently, and continue minor
versions past 0.9. Never bump a version or cut a release until the user says
so. Release 1.0 only with the user's agreement.

### Changelog
`CHANGELOG.md` follows [Keep a Changelog](https://keepachangelog.com). It
describes what changed between releases. Git history already records how the
work got there.
- Have one section per release (`## [0.1.0] - YYYY-MM-DD`). Work in progress
  goes under `## [Unreleased]`.
- Within a release, use only the subsections that have entries: `Added`,
  `Changed`, `Fixed`, plus `Removed` when needed.
- Update the changelog in the same commit as the change it describes.
- Don't pile up entries. If something you add, change or fix was itself added
  in the unreleased version, rewrite its existing entry instead of adding a
  new one. `Changed` and `Fixed` are only for behavior that shipped in an
  earlier release.
- Released sections are frozen. A correction to something released goes into
  the next release.
- Agent notes such as this file don't need changelog entries.

### Merging
**Small changes on `dev`:** fast-forward `main` to `dev`, so the two match. No
PR. When pushing is allowed, push both.

**Significant changes on a branch:** when the user says "merge" (unless they
say to merge directly without a PR):
1. Check that `[Unreleased]` covers everything on the branch, and commit it.
2. Run the local checks under Testing.
3. Push the branch and open a PR into `main`. Write the description as what
   the branch does and why.
4. Wait for CI to pass, then merge.
5. Fast-forward `dev` to `main` so they match, locally and on the remote.
6. Delete the merged branch locally and remotely, unless it still has an open
   PR.

A prior no-push instruction does not prohibit this workflow once a merge is
requested. Check for unrelated changes and divergence first. Never force-push
or overwrite unrelated work.

### Branch cleanup
Before deleting branches, check open pull requests for each local and remote
branch. Never delete a branch associated with an open pull request as part of
branch cleanup; preserve it even when asked to delete other branches.
