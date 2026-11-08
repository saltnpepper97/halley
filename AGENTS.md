# Merge workflow

When the user says to merge a change, publish the feature branch and create a pull request, merge the pull request, then synchronize local and remote main and dev where they exist, unless the user instructs otherwise. Check for unrelated changes and divergence first. Never force-push or overwrite unrelated work. A prior no-push instruction does not prohibit this workflow once a merge is requested.

# Branch naming

Never use `codex/` or a Codex-derived branch prefix. Use descriptive conventional prefixes such as `fix/`, `feat/`, or `release/`.

# Branch cleanup

Before deleting branches, check open pull requests for each local and remote branch. Never delete a branch associated with an open pull request as part of branch cleanup; preserve it even when asked to delete other branches.

# Commits and versioning

Commit incrementally unless the user instructs otherwise. Use MAJOR.MINOR.PATCH, version components independently, and continue minor versions past 0.9. Release 1.0 only with the user's agreement.

# Core design invariants

Preserve these behaviors when changing or refactoring Halley unless the user instructs otherwise. If a change appears to require a different design, discuss it with the user before changing the invariant.

- **Fullscreen and window switching:** When switching away from a fullscreen game to another window on the same monitor, pause its fullscreen state without exiting fullscreen; restore it when returning. Switching to a window on another monitor must leave the game fullscreen on its original monitor. A regular window made fullscreen with Mod+F, such as a browser playing fullscreen YouTube, should instead leave fullscreen when switching away to another window on the same monitor.
- **Pinned nodes:** Pinning exempts a window from automatic decay. It must still be possible to manually convert it to a node and restore it from a node.
- **Empty clusters:** An empty cluster must show its close hint regardless of cursor position or monitor focus. The hint should explain that Super+Q, followed by Super+Q again, deletes the cluster. Preserve the existing cluster design and behavior unless the user requests a change.
- **Closing collapsed windows:** A window collapsed into a node must remain closable with Mod+Q when its node is focused, including when hover focus selects it.
- **XWayland:** Preserve proper EWMH compliance when implementing or changing XWayland behavior.

# Upstream dependencies

Smithay and smithay-drm-extras must use the same explicitly pinned revision of the unmodified official `https://github.com/Smithay/smithay.git` repository. Keep the revision in Cargo.toml and Cargo.lock consistent; update the pin deliberately when needed. Never vendor or bundle Smithay in the repository or release archives, copy its implementation into Halley, use a modified fork, or apply dependency patches or repository source replacements. Implement Halley policy in Halley through public upstream APIs. Do not weaken IME lock isolation or recovery to accommodate a dependency change. The upstream-source CI guard must pass before merging.

Do not place a cloned or modified Smithay source tree anywhere in the project. Keep the current upstream pin unless there is a deliberate reason to update it; Halley currently needs a revision newer than Smithay 0.7 for blur support.

# Testing

Run the local CI checks before merging: the Smithay source guard, `cargo fmt --check`, clippy with `-D warnings` for all three feature sets, workspace tests, and the docs build. Build and install with `cargo build --release --workspace`; without `--workspace` only `halley` is rebuilt, not `halleyctl` or the portal.

Do not run the nested compositor suite (`HALLEY_TEST_WINIT_SERVER=… cargo test --test xwayland_disconnect -- --ignored`, about four minutes) for every change. Run it only when a change affects behaviour that needs a live compositor to observe: window, fullscreen, maximize, focus, pointer or keyboard handling, XWayland, rendering, or capture. Config, docs, CLI and pure-refactor changes rely on CI's headless XWayland job instead. The nested windows open on the user's real desktop, so pointer-sensitive tests can fail while the user is moving the mouse; rerun those rather than treating one failure as a regression.
