# Halley architecture

Halley uses balanced subsystem boundaries: modules are large enough to keep a
complete policy visible, but platform, presentation, shell UI, and protocol
state do not share implementation files.

## Dependency direction

- `session` owns compositor policy and coordinates subsystems.
- `session::RuntimeSettings`, `session::InteractionState`, and
  `shell::ShellState` own configuration reload, compositor-owned input
  transactions, and renderer-independent shell state respectively. They keep
  those lifecycles out of the flat session coordinator.
- `backend` owns only TTY/winit, output, DMA-BUF, frame submission, DPMS, and
  VRR mechanics. Backends implement segregated render, output, and lifecycle
  driver contracts. TTY output and DMA-BUF policy live below `backend::tty`.
- `presentation` owns camera transforms plus shared window and field-maximize
  presentation state used by rendering and input mapping.
- `render` owns scene composition and dedicated persistent GPU resources for
  backgrounds, effects, decorations, textures, text, nodes, and shell UI.
- `window` owns backend-neutral managed-window rules and initial routing.
  Wayland and XWayland supply identities and apply the resolved policy without
  maintaining separate rule engines.
- `shell` owns renderer-independent overlay, bearings, focus-cycle, and Apogee
  state. Their GLES presentation lives below `render`.
- `capture` owns screenshot selection, a separate portal source-chooser state
  machine, pixel capture, and screencast buffers. Screenshot and portal chrome
  have independent render modules and share only capture icon assets.
- `wayland` and `xwayland` own protocol transactions. XWayland control
  properties, managed state, configure constraints, lifecycle,
  override-redirect policy, and presentation policy are separate. X11
  fullscreen selection is a pure policy object; event handling only applies
  its result.
- `session::input` owns raw device-event coordination. Its dispatcher orders
  lock, modal, touch and gesture interception before capture, screenshot,
  pointer grabs, client delivery, and keyboard dispatch. Modal, pointer motion,
  button, wheel, keyboard, and configured action handling live in separate
  modules. Button ownership and suppressed releases stay with the interaction
  that claimed the press; client grabs retain their forwarding path.
  Field and cluster hit testing lives in `session::input::hit_test`; cluster
  activation, creation, and bloom coordination live in `cluster_interaction`.
- `session::cluster_runtime` reconciles cluster state with live surfaces and
  outputs. `session::window_presentation` coordinates fullscreen and maximize
  handoffs, while `session::node_controls` owns focused-node pin controls.
  The `session/` source tree groups startup, onboarding, input, workspace,
  windows, display, capture, protocol, drivers, and diagnostics by their work.
  Explicit source paths in `session/mod.rs` retain the existing Rust module
  boundaries; the folders organize files without changing ownership.
- Cluster activation, window presentation, and workspace dragging are grouped
  in their respective `clusters` modules. Capture request setup lives in
  `capture::requests`; the Wayland output-control protocols are grouped under
  the `wayland/output/protocols` directory. Wayland capture, data transfer,
  desktop, input, output, session, and surface implementations each have their
  own folder; established module paths remain available to callers.
- Fullscreen request handling separates native and external clients, while
  their transaction helpers live alongside shared presentation state. Scene
  entry points apply capture privacy and output scaling before assembly.
- `animation` owns pure timelines. Rendering consumes animation output but
  animation code does not know about GLES or backends.
- `halley-cli` separates command parsing, help, response presentation,
  configuration diagnostics, and IPC transport. Node parsing has its own
  command module; the executable entry point only dispatches typed actions.

One `RenderState` owns persistent GPU caches. A frame crosses the
session/render boundary through six contexts: frame, desktop, cursor, overlays,
visual configuration, and mutable render resources.

Pointer confinement and locking are a protected boundary. Their lifecycle and
motion algorithms remain in `session::pointer::constraints`; presentation
adapters may be called by that subsystem, but rendering and refactors must not
own or duplicate constraint policy.

## Architecture assessment

Architecture quality is assessed per change rather than inferred from file
counts or directory names. Review cohesion (whether a component has one clear
job), coupling (which concrete state and behavior it needs), and maintainability
(how safely a developer can locate, change, and validate that behavior).

The session coordinator remains a shared integration point for Smithay
protocol handlers. Screenshot encoding jobs own their worker and pending replies;
startup owns launch environment and autostart state. Rendering uses a narrow
closing-scene context and separate replacement scene and shared overlay stages.
The remaining ordinary scene assembler, input coordination, and TTY drivers are
still substantial and warrant careful review before further decomposition.

Workspace tests, software-rendered pixel checks, no-default-feature builds, and
strict Clippy are complementary: a passing unit test suite does not cover every
GPU, Wayland desktop, and XWayland integration path.

## SOLID application

- Single responsibility: runtime settings, interaction state, shell state,
  keyboard routing, action routing, and X11 presentation policy have separate
  owners.
- Open/closed and substitution: fullscreen decisions and backend capabilities
  are selected through typed policy/driver contracts; new policy cases or
  backends do not require duplicating the session event loops.
- Interface segregation: render/DMA-BUF, output hardware, and session lifecycle
  are separate driver traits.
- Dependency inversion: backend-independent session policy depends on those
  contracts, while TTY and winit provide the concrete mechanics.

## Change gates

Before merging architectural or rendering changes:

1. `cargo fmt --all -- --check`
2. `cargo test --workspace`
3. `cargo build -p halley`
4. `cargo clippy --workspace --all-targets -- -D warnings`

Configuration syntax and IPC enum ordering are compatibility surfaces. The current private IPC wire version is defined by `HALLEY_IPC_VERSION` in
`halley-ipc/src/types.rs`; change it only for a deliberate wire-format change.
