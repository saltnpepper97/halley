# Merge workflow

When Dustin says to merge a change, publish the feature branch and create a pull request, merge the pull request, then synchronize local and remote main and dev where they exist. Check for unrelated changes and divergence first. Never force-push or overwrite unrelated work. A prior no-push instruction does not prohibit this workflow once a merge is requested.

# Branch naming

Never use `codex/` or a Codex-derived branch prefix. Use descriptive conventional prefixes such as `fix/`, `feat/`, or `release/`.

# Commits and versioning

Commit incrementally unless Dustin instructs otherwise. Use MAJOR.MINOR.PATCH, version components independently, and continue minor versions past 0.9. Release 1.0 only with Dustin's agreement.

# Upstream dependencies

Smithay and smithay-drm-extras must use the same explicitly pinned revision of the unmodified official `https://github.com/Smithay/smithay.git` repository. Keep the revision in Cargo.toml and Cargo.lock consistent; update the pin deliberately when needed. Never vendor or bundle Smithay in the repository or release archives, copy its implementation into Halley, use a modified fork, or apply dependency patches or repository source replacements. Implement Halley policy in Halley through public upstream APIs. Do not weaken IME lock isolation or recovery to accommodate a dependency change. The upstream-source CI guard must pass before merging.
