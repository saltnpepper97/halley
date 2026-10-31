# Contributing

## Required CI checks

All three CI workflows must pass on the latest revision before a pull request
or branch is merged:

| Workflow | Required checks |
| --- | --- |
| [Rust](.github/workflows/rust.yml) | Formatting and workflow lint; strict Clippy and documentation; workspace tests, strict Clippy, release builds and binary smoke tests for default, minimal and dinit features |
| [Nix](.github/workflows/nix.yml) | Package build, workspace tests, installed-resource checks and NixOS module checks on both x86_64-linux and aarch64-linux |
| [XWayland](.github/workflows/xwayland.yml) | All seven headless XWayland regressions |

All three workflows run on pull requests and on pushes to `main` and `dev`,
including documentation-only changes. A feature branch is checked when its
pull request opens or changes, rather than also running the same workflows on
its push. Branches without a pull request can use manual workflow dispatch.
Do not add path filters that bypass these checks. Fix failing checks and wait
for the new runs after pushing changes. Pending, failed, cancelled or skipped
checks do not count as passing, and results from an older revision do not
satisfy this requirement.

See [Testing](docs/testing.md) for the pinned Rust toolchain, local commands,
headless test setup and hardware testing boundaries. Passing CI does not replace
live GPU, DRM/KMS or real-application testing when a change affects those paths.

Other ignored graphical tests, manual visual previews, toolkit checks and
physical hardware tests should be run when relevant to the change. They are
not suitable for every automated push; the bounded headless XWayland suite
remains mandatory.
