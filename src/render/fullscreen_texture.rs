use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;

use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::backend::renderer::{Renderer, Texture};
use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::window_texture::{ResizeCaptureState, ResizeWindowTexture, prepare_resize_capture};

#[derive(Debug)]
struct TransitionTextures {
    id: Id,
    previous: ResizeWindowTexture,
    /// Incoming endpoint, refreshed when its rendered surface tree changes.
    /// The owning transaction authorizes a validated endpoint before blending;
    /// the live tree stays out of the scene for the rest of the crossfade.
    next: Option<ResizeWindowTexture>,
    /// Scratch endpoint used for validation. It becomes `next` only if the
    /// owning transaction accepts the same commit. Mixed Firefox surface-tree
    /// commits never overwrite the displayed endpoint before that.
    prepared: Option<ResizeWindowTexture>,
    /// Last attempted tree, including rejected mixed endpoints. Retrying an
    /// identical tree cannot produce a different snapshot or readiness result.
    last_capture: Option<ResizeCaptureState>,
    owner: TextureTransitionOwner,
    last_surface_commit: CommitCounter,
    outgoing_surface_size: Option<Size<i32, Logical>>,
    requires_fresh_damage: bool,
    target_readiness: TargetReadiness,
    target_damage: TargetDamageCoverage,
    capture_generation: CommitCounter,
    /// When the accepted endpoint first became presentable. The crossfade
    /// runs from there, so a late client buffer fades in instead of popping
    /// to the already-advanced geometry progress.
    blend_start: Option<BlendStart>,
}

/// Wall-clock length of the endpoint crossfade. Spreading the fade over the
/// geometry motion kept two differently stretched images on screen for most
/// of it, including the slow tail of a spring. A short fixed fade keeps the
/// double image to a few frames regardless of the motion's feel or length.
const CROSSFADE_DURATION: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug)]
struct BlendStart {
    progress: f64,
    time: Duration,
}

impl BlendStart {
    /// Linear fade position. The geometry term guarantees the fade has
    /// finished when the motion does, even for very short transitions.
    fn fade(self, progress: f64, now: Duration) -> f64 {
        let elapsed = now.saturating_sub(self.time).as_secs_f64();
        let timed = (elapsed / CROSSFADE_DURATION.as_secs_f64()).clamp(0.0, 1.0);
        timed.max(crossfade_progress(self.progress, progress))
    }
}

#[derive(Debug, Default)]
struct TargetDamageCoverage {
    size: Option<Size<i32, Buffer>>,
    // One bit per pixel bounds storage by the current buffer, not commit
    // history. Most clients repaint the whole buffer and need no bitmap.
    painted: Vec<u64>,
    painted_pixels: usize,
    allocation_failed: bool,
}

/// Repaint evidence is not transaction authority.
///
/// Native clients such as Firefox routinely commit subsurfaces and stale root
/// buffers before acknowledging a resize configure. Those commits can be
/// candidates, but rendering must keep holding the outgoing snapshot until
/// fullscreen/maximize explicitly authorizes the accepted resize commit.
#[derive(Debug, Default)]
struct TargetReadiness {
    candidate: bool,
    authorized: bool,
}

impl TargetReadiness {
    fn observe_candidate(&mut self, ready: bool) {
        self.candidate = ready;
    }

    fn authorize(&mut self) {
        debug_assert!(self.candidate);
        self.authorized = true;
    }
}

impl TargetDamageCoverage {
    fn observe(
        &mut self,
        size: Option<Size<i32, Buffer>>,
        damage: &[Rectangle<i32, Buffer>],
    ) -> bool {
        if self.size != size {
            *self = Self {
                size,
                ..Self::default()
            };
        }
        let Some(size) = size.filter(|size| size.w > 0 && size.h > 0) else {
            return false;
        };
        let (width, height) = (size.w as usize, size.h as usize);
        let word_bits = u64::BITS as usize;
        let Some(total) = width.checked_mul(height) else {
            return false;
        };
        if self.painted_pixels == total {
            return true;
        }
        let clipped = damage.iter().filter_map(|rect| {
            let x1 = rect.loc.x.clamp(0, size.w) as usize;
            let y1 = rect.loc.y.clamp(0, size.h) as usize;
            let x2 = rect.loc.x.saturating_add(rect.size.w).clamp(0, size.w) as usize;
            let y2 = rect.loc.y.saturating_add(rect.size.h).clamp(0, size.h) as usize;
            (x2 > x1 && y2 > y1).then_some((x1, y1, x2, y2))
        });
        if clipped.clone().any(|rect| rect == (0, 0, width, height)) {
            self.painted_pixels = total;
            self.painted = Vec::new();
            return true;
        }
        if self.allocation_failed {
            return false;
        }
        for (x1, y1, x2, y2) in clipped {
            if self.painted.is_empty() {
                let words = total.div_ceil(word_bits);
                if self.painted.try_reserve_exact(words).is_err() {
                    // Do not mistake missing coverage evidence for a repaint.
                    // A subsequent full-buffer damage can still complete it.
                    self.allocation_failed = true;
                    return false;
                }
                self.painted.resize(words, 0);
            }
            for y in y1..y2 {
                let start = y * width + x1;
                let end = y * width + x2;
                let first = start / word_bits;
                let last = (end - 1) / word_bits;
                for word in first..=last {
                    let lo = if word == first { start % word_bits } else { 0 };
                    let hi = if word == last {
                        (end - 1) % word_bits + 1
                    } else {
                        word_bits
                    };
                    let mask = (u64::MAX << lo) & (u64::MAX >> (word_bits - hi));
                    let fresh = mask & !self.painted[word];
                    self.painted_pixels += fresh.count_ones() as usize;
                    self.painted[word] |= mask;
                }
            }
            if self.painted_pixels == total {
                self.painted = Vec::new();
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureTransitionOwner {
    Fullscreen,
    Maximize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRepaint {
    pub owner: TextureTransitionOwner,
    pub ready: bool,
}

#[derive(Default)]
pub struct FullscreenTextureTransitions {
    windows: HashMap<WlSurface, TransitionTextures>,
    resize_renderer: super::resize::ResizeRenderer,
}

/// What the scene presents for a window during its texture transition.
pub enum TransitionFrame {
    /// Both endpoints are snapshots. The live tree stays out of the scene.
    Snapshot(super::resize::ResizeRenderElement),
    /// The live tree is a complete target endpoint and is presented directly,
    /// so moving content keeps updating every frame. `outgoing` is the held
    /// snapshot fading out above it, or `None` once the fade has finished.
    Live {
        outgoing: Option<super::resize::ResizeRenderElement>,
    },
}

/// One window's crossfade between its captured and live textures.
#[derive(Clone, Copy)]
pub struct BlendRequest<'a> {
    pub window: &'a Window,
    pub destination: Rectangle<i32, Physical>,
    pub progress: f64,
    /// X11 applies restored fullscreen-exit geometry before the matching
    /// `wl_surface` buffer necessarily arrives. Capturing the still-fullscreen
    /// buffer at the smaller target crops it, making the reverse animation look
    /// like a discontinuous shrink. Set this to keep presenting the intact
    /// captured texture until XWayland commits the restored client size;
    /// geometry continues to animate independently through `destination`.
    pub hold_previous_until_restored_buffer_matches: bool,
    pub alpha: f32,
    pub radii: super::window_decoration::CornerRadii,
    /// Presentation time of the frame being rendered; drives the crossfade.
    pub now: Duration,
}

impl FullscreenTextureTransitions {
    /// The live surface stays out of the scene for the whole crossfade, not
    /// only while the first incoming snapshot is missing. Frame callbacks must
    /// still reach that client so it can finish painting offscreen.
    pub fn awaiting_target(&self, surface: &WlSurface) -> bool {
        self.windows.contains_key(surface)
    }

    pub fn capture_previous(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        owner: TextureTransitionOwner,
    ) -> Result<(), Box<dyn Error>> {
        let surface = window
            .wl_surface()
            .ok_or("fullscreen snapshot window has no surface")?
            .into_owned();
        self.windows.remove(&surface);
        let previous = super::window_texture::capture_for_resize(renderer, window, None)?;
        eventline::debug!(
            "window transition: captured outgoing owner={owner:?} window={:?} surface={:?} opaque={}",
            previous.window_size,
            previous.surface_geometry,
            previous.client_opaque,
        );
        let (last_surface_commit, outgoing_surface_size) =
            with_renderer_surface_state(&surface, |state| {
                (state.current_commit(), state.surface_size())
            })
            .unwrap_or_default();
        self.windows.insert(
            surface,
            TransitionTextures {
                id: Id::new(),
                previous,
                next: None,
                prepared: None,
                last_capture: None,
                owner,
                last_surface_commit,
                outgoing_surface_size,
                requires_fresh_damage: crate::xwayland::is_x11(window),
                target_readiness: TargetReadiness::default(),
                target_damage: TargetDamageCoverage::default(),
                capture_generation: CommitCounter::default(),
                blend_start: None,
            },
        );
        Ok(())
    }

    /// Records the damage boundary of each surface commit while a resize
    /// transaction is pending.
    ///
    /// Xwayland can attach the correctly-sized allocation before the X11
    /// client has painted it. Size alone therefore does not make a target
    /// endpoint safe to capture. Updating the baseline on every commit also
    /// prevents unrelated damage on the outgoing buffer from being mistaken
    /// for the target repaint.
    pub fn observe_surface_commit(&mut self, surface: &WlSurface) -> Option<TargetRepaint> {
        let entry = self.windows.get_mut(surface)?;
        if entry.target_readiness.authorized {
            // The owning protocol transaction is complete. Later commits are
            // ordinary live updates consumed by blend_element(); they must not
            // re-enter or block fullscreen/maximize state settlement.
            return None;
        }
        // A prepared endpoint belongs to one exact commit. If the protocol
        // manager did not accept it, never carry it across a later commit.
        entry.prepared = None;
        let (current, damage, buffer_size, surface_size) =
            with_renderer_surface_state(surface, |state| {
                (
                    state.current_commit(),
                    state.damage_since(Some(entry.last_surface_commit)),
                    state
                        .buffer_size()
                        .map(|size| size.to_buffer(state.buffer_scale(), state.buffer_transform())),
                    state.surface_size(),
                )
            })
            .unwrap_or((entry.last_surface_commit, Default::default(), None, None));
        let advanced = commit_has_advanced(current, entry.last_surface_commit);
        let candidate_ready = if !entry.requires_fresh_damage {
            let committed_window_size = with_states(surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
                    .map(|geometry| geometry.size)
            });
            let endpoint_ready = native_target_buffer_ready(
                advanced,
                entry.previous.window_size,
                entry.outgoing_surface_size,
                committed_window_size,
                surface_size,
            );
            if advanced {
                eventline::debug!(
                    "window transition: native repaint candidate ready={endpoint_ready} outgoing-window={:?} outgoing-root={:?} committed-window={:?} current-root={:?}",
                    entry.previous.window_size,
                    entry.outgoing_surface_size,
                    committed_window_size,
                    surface_size,
                );
            }
            // Firefox repaints its root and decoration/content subsurfaces in
            // separate commits. Once a new root allocation is a candidate,
            // keep retrying the complete surface-tree capture on subsequent
            // child commits. Those commits re-enter with the root commit
            // counter unchanged; revoking the candidate here left the
            // fullscreen transaction holding its outgoing frame forever.
            native_target_candidate_ready(
                entry.target_readiness.candidate,
                advanced,
                endpoint_ready,
            )
        } else if advanced {
            entry.target_damage.observe(buffer_size, &damage)
        } else {
            // Subsurface commits re-enter this root-surface observer without
            // advancing the root commit counter. Do not revoke repaint
            // evidence that was already complete for the current allocation.
            entry.target_readiness.candidate
        };
        entry.last_surface_commit = current;
        entry.target_readiness.observe_candidate(candidate_ready);
        Some(TargetRepaint {
            owner: entry.owner,
            ready: entry.target_readiness.candidate,
        })
    }

    /// Captures and validates the complete surface tree for the candidate
    /// commit without yet making it renderable.
    ///
    /// Firefox commits its resized root buffer before all of its decoration
    /// subsurfaces have retired the outgoing allocation. The root size and xdg
    /// geometry are correct at that point, but a surface-tree snapshot is
    /// still mostly the old fullscreen texture. Classify the complete extent
    /// in client coordinates, accounting for decoration margins, and keep
    /// holding the outgoing snapshot until its tree matches the target.
    pub fn prepare_target(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        owner: TextureTransitionOwner,
    ) -> Result<bool, Box<dyn Error>> {
        let surface = window
            .wl_surface()
            .ok_or("resize target window has no surface")?;
        let context = renderer.context_id();
        let Some(entry) = self.windows.get_mut(surface.as_ref()) else {
            return Ok(false);
        };
        if entry.owner != owner || !entry.target_readiness.candidate || entry.next.is_some() {
            return Ok(false);
        }
        if entry.previous.context != context {
            self.windows.remove(surface.as_ref());
            return Ok(false);
        }
        let capture = prepare_resize_capture(renderer, window)?;
        let state = capture.state.clone();
        let candidate = capture.capture(renderer, None)?;
        if !snapshot_matches_target_endpoint(&entry.previous, &candidate) {
            eventline::debug!(
                "window transition: rejected mixed surface-tree endpoint owner={owner:?} outgoing-window={:?} candidate-window={:?} candidate-surface={:?} opaque={}",
                entry.previous.window_size,
                candidate.window_size,
                candidate.surface_geometry,
                candidate.client_opaque,
            );
            return Ok(false);
        }
        entry.prepared = Some(candidate);
        entry.last_capture = Some(state);
        Ok(true)
    }

    /// Freezes the painted target while handling the authoritative surface
    /// commit, before a later XWayland commit can replace its contents.
    pub fn capture_target(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &Window,
        owner: TextureTransitionOwner,
    ) -> Result<bool, Box<dyn Error>> {
        let surface = window
            .wl_surface()
            .ok_or("resize target window has no surface")?
            .into_owned();
        let context = renderer.context_id();
        let Some(entry) = self.windows.get(&surface) else {
            return Ok(false);
        };
        if entry.owner != owner || !entry.target_readiness.candidate || entry.next.is_some() {
            return Ok(false);
        }
        if entry.previous.context != context {
            self.windows.remove(&surface);
            return Ok(false);
        }
        if entry.prepared.is_none() && !self.prepare_target(renderer, window, owner)? {
            return Ok(false);
        }
        let entry = self
            .windows
            .get_mut(&surface)
            .expect("resize transition checked above");
        let next = entry
            .prepared
            .take()
            .expect("prepared target checked above");
        // Rendering is allowed to sample the candidate only after the
        // presentation state machine has accepted this exact commit.
        if !entry.target_readiness.authorized {
            entry.target_readiness.authorize();
        }
        eventline::debug!(
            "window transition: accepted fully repainted target owner={owner:?} size={:?} opaque={}",
            next.texture.size(),
            next.client_opaque,
        );
        entry.next = Some(next);
        entry.capture_generation.increment();
        Ok(true)
    }

    pub fn remove(&mut self, surface: &WlSurface) {
        self.windows.remove(surface);
    }

    pub fn remove_owner(&mut self, owner: TextureTransitionOwner) {
        self.windows.retain(|_, entry| entry.owner != owner);
    }

    pub fn blend_element(
        &mut self,
        renderer: &mut GlesRenderer,
        request: BlendRequest<'_>,
    ) -> Result<Option<TransitionFrame>, Box<dyn Error>> {
        let BlendRequest {
            window,
            destination,
            progress,
            hold_previous_until_restored_buffer_matches,
            alpha,
            radii,
            now,
        } = request;
        let surface = window
            .wl_surface()
            .ok_or("fullscreen blend window has no surface")?;
        let context = renderer.context_id();
        let Some(entry) = self.windows.get_mut(surface.as_ref()) else {
            return Ok(None);
        };
        if entry.previous.context != context {
            self.windows.remove(surface.as_ref());
            return Ok(None);
        }

        let buffer_matches = entry.target_readiness.authorized
            && transition_buffer_ready(
                hold_previous_until_restored_buffer_matches,
                with_renderer_surface_state(surface.as_ref(), |state| state.surface_size())
                    .flatten(),
                window.geometry().size,
            );
        let (next, texture_progress) = if buffer_matches {
            let capture = match prepare_resize_capture(renderer, window) {
                Ok(capture) => capture,
                Err(err) => {
                    self.windows.remove(surface.as_ref());
                    return Err(err);
                }
            };
            // A re-captured endpoint only changes when the client commits a
            // tree that passes validation, so moving content stutters. Once
            // the live tree itself is complete, present it directly and fade
            // the held outgoing snapshot out above it.
            let live_complete = capture.layout().is_ok_and(|layout| {
                layout_matches_target_endpoint(
                    &entry.previous,
                    layout.surface_geometry,
                    layout.window_size,
                    &layout.surface_layers,
                )
            });
            if live_complete {
                let fade = entry
                    .blend_start
                    .get_or_insert(BlendStart {
                        progress,
                        time: now,
                    })
                    .fade(progress, now);
                let outgoing = if fade < 1.0 {
                    Some(self.resize_renderer.element(
                        renderer,
                        entry.id.clone(),
                        &entry.previous,
                        entry.previous.clone(),
                        destination,
                        0.0,
                        alpha * (1.0 - crossfade_weight(fade)) as f32,
                        radii,
                        entry.capture_generation,
                    )?)
                } else {
                    None
                };
                return Ok(Some(TransitionFrame::Live { outgoing }));
            }
            if entry.last_capture.as_ref() != Some(&capture.state) {
                let state = capture.state.clone();
                let reusable = entry.prepared.take().map(|candidate| candidate.texture);
                let candidate = match capture.capture(renderer, reusable) {
                    Ok(candidate) => candidate,
                    Err(err) => {
                        self.windows.remove(surface.as_ref());
                        return Err(err);
                    }
                };
                if snapshot_matches_target_endpoint(&entry.previous, &candidate) {
                    entry.prepared = entry.next.replace(candidate);
                    entry.capture_generation.increment();
                } else {
                    eventline::debug!(
                        "window transition: retained last complete endpoint owner={:?} candidate-window={:?} candidate-surface={:?} opaque={}",
                        entry.owner,
                        candidate.window_size,
                        candidate.surface_geometry,
                        candidate.client_opaque,
                    );
                    entry.prepared = Some(candidate);
                }
                entry.last_capture = Some(state);
            }
            let next = entry
                .next
                .clone()
                .expect("authorized resize transition has an accepted endpoint");
            // Configure/buffer/tree validation already accepted this endpoint.
            // Opaque regions describe where a client permits occlusion, not
            // whether it has painted. A client may clear that hint at resize.
            let fade = entry
                .blend_start
                .get_or_insert(BlendStart {
                    progress,
                    time: now,
                })
                .fade(progress, now);
            (next, fade)
        } else {
            (
                ResizeWindowTexture {
                    texture: entry.previous.texture.clone(),
                    context: entry.previous.context.clone(),
                    surface_geometry: entry.previous.surface_geometry,
                    window_size: entry.previous.window_size,
                    client_opaque: entry.previous.client_opaque,
                    surface_layers: entry.previous.surface_layers.clone(),
                },
                0.0,
            )
        };
        let id = entry.id.clone();
        Ok(Some(TransitionFrame::Snapshot(
            self.resize_renderer.element(
                renderer,
                id,
                &entry.previous,
                next,
                destination,
                texture_progress as f32,
                alpha,
                radii,
                entry.capture_generation,
            )?,
        )))
    }
}

/// Weight of the incoming endpoint, matching the resize shader's mix so a
/// window can switch between snapshot and live presentation mid-fade.
fn crossfade_weight(fade: f64) -> f64 {
    let t = ((fade - 0.15) / (0.85 - 0.15)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn crossfade_progress(start: f64, progress: f64) -> f64 {
    let remaining = 1.0 - start;
    if remaining <= f64::EPSILON {
        return 1.0;
    }
    ((progress - start) / remaining).clamp(0.0, 1.0)
}

pub(crate) fn snapshot_matches_target_endpoint(
    outgoing: &ResizeWindowTexture,
    candidate: &ResizeWindowTexture,
) -> bool {
    layout_matches_target_endpoint(
        outgoing,
        candidate.surface_geometry,
        candidate.window_size,
        &candidate.surface_layers,
    )
}

fn layout_matches_target_endpoint(
    outgoing: &ResizeWindowTexture,
    surface_geometry: Rectangle<i32, Physical>,
    window_size: Size<i32, Physical>,
    surface_layers: &[(Id, Rectangle<i32, Physical>)],
) -> bool {
    surface_tree_matches_target_endpoint(outgoing.window_size, surface_geometry, window_size)
        && persistent_endpoint_layers_cover_target(
            outgoing.window_size,
            &outgoing.surface_layers,
            window_size,
            surface_layers,
        )
}

/// A merged surface-tree extent is not sufficient resize evidence. Firefox
/// can resize its decoration/shadow subsurface before its persistent content
/// surface; the union then matches the target even though most of the target
/// snapshot still contains the outgoing-sized client. Every persistent layer
/// that covered the outgoing client must either be retired or match the new
/// client before the endpoint is frozen. On a small shrink, resized shadows
/// can enclose unchanged fullscreen content while their union looks correct.
fn persistent_endpoint_layers_cover_target<LayerId: Eq>(
    outgoing_window_size: Size<i32, Physical>,
    outgoing_layers: &[(LayerId, Rectangle<i32, Physical>)],
    candidate_window_size: Size<i32, Physical>,
    candidate_layers: &[(LayerId, Rectangle<i32, Physical>)],
) -> bool {
    let outgoing_client = Rectangle::from_size(outgoing_window_size);
    outgoing_layers
        .iter()
        .filter(|(_, geometry)| rectangle_contains(*geometry, outgoing_client))
        .all(|(id, _)| {
            candidate_layers
                .iter()
                .find(|(candidate_id, _)| candidate_id == id)
                .is_none_or(|(_, geometry)| {
                    surface_tree_matches_target_endpoint(
                        outgoing_window_size,
                        *geometry,
                        candidate_window_size,
                    )
                })
        })
}

fn rectangle_contains<Kind>(container: Rectangle<i32, Kind>, target: Rectangle<i32, Kind>) -> bool {
    container.loc.x <= target.loc.x
        && container.loc.y <= target.loc.y
        && container.loc.x.saturating_add(container.size.w)
            >= target.loc.x.saturating_add(target.size.w)
        && container.loc.y.saturating_add(container.size.h)
            >= target.loc.y.saturating_add(target.size.h)
}

fn surface_tree_matches_target_endpoint(
    outgoing_window_size: Size<i32, Physical>,
    candidate_surface_geometry: Rectangle<i32, Physical>,
    candidate_window_size: Size<i32, Physical>,
) -> bool {
    rectangle_contains(
        candidate_surface_geometry,
        Rectangle::from_size(candidate_window_size),
    ) && endpoint_margin_distance(candidate_surface_geometry, candidate_window_size)
        <= endpoint_margin_distance(candidate_surface_geometry, outgoing_window_size)
}

/// Snapshots use the xdg client origin, so negative tree coordinates describe
/// the left/top CSD margins. Compare the opposite margins around each possible
/// client size instead of comparing the decorated allocation to an undecorated
/// size. Otherwise a lone tile close to fullscreen can be classified as stale
/// forever: a 2520x1350 client with 20px margins has a 2560x1390 tree, closer in
/// raw size to 2560x1440 fullscreen than to its own client geometry.
fn endpoint_margin_distance(
    surface_geometry: Rectangle<i32, Physical>,
    window_size: Size<i32, Physical>,
) -> i64 {
    let left = -i64::from(surface_geometry.loc.x);
    let top = -i64::from(surface_geometry.loc.y);
    let right = i64::from(surface_geometry.loc.x) + i64::from(surface_geometry.size.w)
        - i64::from(window_size.w);
    let bottom = i64::from(surface_geometry.loc.y) + i64::from(surface_geometry.size.h)
        - i64::from(window_size.h);
    (right - left).abs() + (bottom - top).abs()
}

fn commit_has_advanced(current: CommitCounter, previous: CommitCounter) -> bool {
    current
        .distance(Some(previous))
        .is_some_and(|distance| distance > 0)
}

/// A native configure acknowledgement is not enough to identify the resized
/// endpoint. Firefox can commit the new xdg window geometry while continuing
/// to present the old root buffer for another commit. Capturing at that point
/// pairs the restored geometry with a fullscreen allocation, producing the
/// large, faded crop seen during Field fullscreen exits.
///
/// A real resize therefore requires a new root allocation. A state-only edge
/// whose configured window size did not change may legitimately reuse the
/// existing allocation.
fn native_target_buffer_ready(
    advanced: bool,
    outgoing_window_size: Size<i32, Physical>,
    outgoing_surface_size: Option<Size<i32, Logical>>,
    committed_window_size: Option<Size<i32, Logical>>,
    surface_size: Option<Size<i32, Logical>>,
) -> bool {
    // A restored session can leave fullscreen at the same window size. The
    // client may then acknowledge the state change without attaching another
    // buffer. That is a valid endpoint, not a missing repaint. Protocol state
    // is still checked by the owning fullscreen/maximize transaction.
    // With no explicit xdg geometry, an unchanged root can itself describe
    // the client window. Complete surface-tree validation still follows.
    if committed_window_size
        .or(surface_size)
        .map(|size| size.to_physical(1))
        == Some(outgoing_window_size)
        && surface_size.is_some()
        && surface_size == outgoing_surface_size
    {
        return true;
    }
    advanced && surface_size.is_some() && surface_size != outgoing_surface_size
}

/// Root repaint evidence remains valid while Firefox finishes the matching
/// subsurface tree. Only a newer root commit may replace or revoke it.
fn native_target_candidate_ready(
    previous_candidate: bool,
    root_commit_advanced: bool,
    endpoint_ready: bool,
) -> bool {
    endpoint_ready || (!root_commit_advanced && previous_candidate)
}

fn transition_buffer_ready(
    hold_previous: bool,
    buffer_size: Option<Size<i32, Logical>>,
    configured_size: Size<i32, Logical>,
) -> bool {
    !hold_previous || buffer_size == Some(configured_size)
}

#[cfg(test)]
mod tests {
    use super::super::window_texture::rectangles_cover_size as damage_covers_buffer;
    use super::{
        BlendStart, CROSSFADE_DURATION, TargetDamageCoverage, TargetReadiness, commit_has_advanced,
        crossfade_progress, crossfade_weight, native_target_buffer_ready,
        native_target_candidate_ready, persistent_endpoint_layers_cover_target,
        surface_tree_matches_target_endpoint, transition_buffer_ready,
    };
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Buffer, Logical, Physical, Rectangle, Size};

    #[test]
    fn late_endpoint_fades_from_its_acceptance_without_a_jump() {
        for start in [0.0, 0.25, 0.75, 0.99] {
            assert_eq!(crossfade_progress(start, start), 0.0);
            assert!((crossfade_progress(start, start + (1.0 - start) * 0.5) - 0.5).abs() < 1e-12);
            assert_eq!(crossfade_progress(start, 1.0), 1.0);
            assert_eq!(crossfade_progress(start, start - 0.1), 0.0);
        }
        assert_eq!(crossfade_progress(1.0, 1.0), 1.0);
    }

    #[test]
    fn crossfade_is_timed_from_acceptance_and_done_when_motion_is() {
        use std::time::Duration;
        let start = BlendStart {
            progress: 0.4,
            time: Duration::from_millis(1000),
        };
        let now = |ms: u64| Duration::from_millis(1000 + ms);
        assert_eq!(start.fade(0.4, now(0)), 0.0);
        // A slow spring tail does not stretch the fade.
        let half = CROSSFADE_DURATION / 2;
        assert!((start.fade(0.41, Duration::from_millis(1000) + half) - 0.5).abs() < 1e-9);
        assert_eq!(
            start.fade(0.5, Duration::from_millis(1000) + CROSSFADE_DURATION),
            1.0
        );
        // A motion shorter than the fade still finishes it.
        assert_eq!(start.fade(1.0, now(10)), 1.0);
        // A frame timestamped before acceptance never fades backwards.
        assert_eq!(start.fade(0.4, Duration::from_millis(900)), 0.0);
    }

    #[test]
    fn live_fade_weight_matches_the_shader_mix_endpoints() {
        // The resize shader holds each endpoint outside 0.15..0.85. A live
        // fade must agree, or switching presentation mid-fade would jump.
        assert_eq!(crossfade_weight(0.0), 0.0);
        assert_eq!(crossfade_weight(0.15), 0.0);
        assert_eq!(crossfade_weight(0.5), 0.5);
        assert_eq!(crossfade_weight(0.85), 1.0);
        assert_eq!(crossfade_weight(1.0), 1.0);
        let samples = (0..=100).map(|step| crossfade_weight(f64::from(step) / 100.0));
        assert!(
            samples
                .clone()
                .zip(samples.skip(1))
                .all(|(before, after)| after >= before)
        );
    }

    #[test]
    fn x11_fullscreen_exit_holds_previous_texture_until_restored_buffer_arrives() {
        let restored = (1200, 800).into();

        assert!(!transition_buffer_ready(
            true,
            Some((1920, 1080).into()),
            restored
        ));
        assert!(!transition_buffer_ready(true, None, restored));
        assert!(transition_buffer_ready(
            true,
            Some((1200, 800).into()),
            restored
        ));
        assert!(transition_buffer_ready(
            false,
            Some((1920, 1080).into()),
            restored
        ));
    }

    #[test]
    fn x11_target_requires_damage_after_the_last_observed_commit() {
        let previous = CommitCounter::from(41);

        assert!(!commit_has_advanced(previous, previous));
        assert!(commit_has_advanced(CommitCounter::from(42), previous));
    }

    #[test]
    fn repaint_candidate_cannot_be_sampled_before_transaction_authority() {
        let mut readiness = TargetReadiness::default();

        readiness.observe_candidate(true);
        assert!(readiness.candidate);
        assert!(!readiness.authorized);

        readiness.authorize();
        assert!(readiness.authorized);
    }

    #[test]
    fn native_target_rejects_geometry_ack_with_the_outgoing_root_allocation() {
        let outgoing_window = Size::<i32, Physical>::from((2560, 1440));
        let outgoing_surface = Some(Size::<i32, Logical>::from((2560, 1440)));

        assert!(!native_target_buffer_ready(
            true,
            outgoing_window,
            outgoing_surface,
            Some((996, 664).into()),
            outgoing_surface,
        ));
    }

    #[test]
    fn native_target_accepts_the_resized_root_allocation() {
        assert!(native_target_buffer_ready(
            true,
            (2560, 1440).into(),
            Some((2560, 1440).into()),
            Some((996, 664).into()),
            Some((1036, 704).into()),
        ));
    }

    #[test]
    fn native_state_only_target_can_reuse_an_unchanged_allocation() {
        assert!(native_target_buffer_ready(
            true,
            (996, 664).into(),
            Some((1036, 704).into()),
            Some((996, 664).into()),
            Some((1036, 704).into()),
        ));
    }

    #[test]
    fn native_subsurface_commit_cannot_release_the_resize_transaction() {
        assert!(!native_target_buffer_ready(
            false,
            (996, 664).into(),
            Some((1036, 704).into()),
            Some((2560, 1440).into()),
            Some((2560, 1440).into()),
        ));
    }

    #[test]
    fn restored_fullscreen_can_exit_with_a_state_only_commit() {
        let window = (2560, 1440).into();
        let root = Some((2560, 1440).into());
        let ready = native_target_buffer_ready(false, window, root, root, root);
        assert!(ready);
        assert!(native_target_candidate_ready(false, false, ready));
    }

    #[test]
    fn state_only_endpoint_without_explicit_xdg_geometry_can_keep_its_root() {
        let root = Some((800, 600).into());
        assert!(native_target_buffer_ready(
            false,
            (800, 600).into(),
            root,
            None,
            root
        ));
        assert!(!native_target_buffer_ready(
            false,
            (800, 600).into(),
            root,
            None,
            None
        ));
    }

    #[test]
    fn state_only_commit_cannot_approve_a_resize_with_the_old_buffer() {
        let root = Some((2560, 1440).into());
        assert!(!native_target_buffer_ready(
            false,
            (2560, 1440).into(),
            root,
            Some((996, 664).into()),
            root,
        ));
        assert!(!native_target_buffer_ready(
            false,
            (2560, 1440).into(),
            None,
            root,
            None,
        ));
    }

    #[test]
    fn native_subsurface_commit_retains_a_root_repaint_candidate() {
        assert!(native_target_candidate_ready(true, false, false));
    }

    #[test]
    fn native_subsurface_commit_cannot_invent_a_root_repaint_candidate() {
        assert!(!native_target_candidate_ready(false, false, false));
    }

    #[test]
    fn newer_native_root_commit_can_revoke_a_stale_repaint_candidate() {
        assert!(!native_target_candidate_ready(true, true, false));
    }

    #[test]
    fn native_target_rejects_outgoing_sized_subsurfaces_after_root_resize() {
        assert!(!surface_tree_matches_target_endpoint(
            (1280, 800).into(),
            Rectangle::new((-10, -10).into(), (1300, 820).into()),
            (500, 304).into(),
        ));
    }

    #[test]
    fn native_target_accepts_resized_surface_tree_with_csd_margins() {
        assert!(surface_tree_matches_target_endpoint(
            (1280, 800).into(),
            Rectangle::new((-20, -20).into(), (540, 344).into()),
            (500, 304).into(),
        ));
    }

    #[test]
    fn single_window_fullscreen_exit_accepts_the_restored_csd_tree() {
        // Recorded GTK/Firefox exit: the restored 2520x1350 client has 20px
        // margins, making its full tree width equal to the outgoing fullscreen.
        assert!(surface_tree_matches_target_endpoint(
            (2560, 1440).into(),
            Rectangle::new((-20, -20).into(), (2560, 1390).into()),
            (2520, 1350).into(),
        ));
    }

    #[test]
    fn nearby_fullscreen_exit_accepts_csd_margins_when_only_one_axis_resizes() {
        for client in [(2520, 1440), (2560, 1400)] {
            assert!(surface_tree_matches_target_endpoint(
                (2560, 1440).into(),
                Rectangle::new((-20, -20).into(), (client.0 + 40, client.1 + 40).into()),
                client.into(),
            ));
        }
    }

    #[test]
    fn nearby_fullscreen_exit_accepts_asymmetric_csd_margins() {
        assert!(surface_tree_matches_target_endpoint(
            (2560, 1440).into(),
            Rectangle::new((-20, -18).into(), (2564, 1396).into()),
            (2520, 1350).into(),
        ));
    }

    #[test]
    fn nearby_fullscreen_exit_rejects_outgoing_sized_and_mixed_trees() {
        // Both the entirely stale decorated tree and an old fullscreen
        // content layer behind resized decorations must keep the exit held.
        for surface in [(2600, 1480), (2580, 1460)] {
            assert!(!surface_tree_matches_target_endpoint(
                (2560, 1440).into(),
                Rectangle::new((-20, -20).into(), surface.into()),
                (2520, 1350).into(),
            ));
        }
    }

    #[test]
    fn native_target_rejects_a_tree_that_does_not_cover_the_target_client() {
        assert!(!surface_tree_matches_target_endpoint(
            (500, 304).into(),
            Rectangle::new((0, 0).into(), (1270, 790).into()),
            (1280, 800).into(),
        ));
        assert!(!surface_tree_matches_target_endpoint(
            (2560, 1440).into(),
            Rectangle::new((20, 20).into(), (2520, 1350).into()),
            (2520, 1350).into(),
        ));
    }

    #[test]
    fn native_target_rejects_mixed_persistent_surface_layers() {
        let outgoing_layers = [
            (1, Rectangle::new((0, 0).into(), (252, 304).into())),
            (2, Rectangle::new((-20, -20).into(), (292, 344).into())),
        ];
        let mixed_target_layers = [
            (1, Rectangle::new((0, 0).into(), (252, 304).into())),
            (2, Rectangle::new((0, 0).into(), (1280, 800).into())),
        ];

        assert!(!persistent_endpoint_layers_cover_target(
            (252, 304).into(),
            &outgoing_layers,
            (1280, 800).into(),
            &mixed_target_layers,
        ));
    }

    #[test]
    fn native_shrink_rejects_old_content_hidden_within_restored_csd_margins() {
        let outgoing = (2560, 1440).into();
        let restored = (2540, 1440).into();
        let outgoing_layers = [
            (1, Rectangle::from_size(outgoing)),
            (2, Rectangle::new((-20, -20).into(), (2600, 1480).into())),
        ];
        let restored_shadow = Rectangle::new((-20, -20).into(), (2580, 1480).into());
        let mixed_layers = [(1, Rectangle::from_size(outgoing)), (2, restored_shadow)];

        // The resized shadow already encloses the outgoing content, so the
        // union alone cannot distinguish this partial commit from completion.
        assert!(surface_tree_matches_target_endpoint(
            outgoing,
            restored_shadow,
            restored,
        ));
        assert!(!persistent_endpoint_layers_cover_target(
            outgoing,
            &outgoing_layers,
            restored,
            &mixed_layers,
        ));

        let complete_layers = [(1, Rectangle::from_size(restored)), (2, restored_shadow)];
        assert!(persistent_endpoint_layers_cover_target(
            outgoing,
            &outgoing_layers,
            restored,
            &complete_layers,
        ));
        let replaced_layers = [(3, Rectangle::from_size(restored)), (2, restored_shadow)];
        assert!(persistent_endpoint_layers_cover_target(
            outgoing,
            &outgoing_layers,
            restored,
            &replaced_layers,
        ));
    }

    #[test]
    fn native_target_accepts_fully_resized_persistent_surface_layers() {
        let outgoing_layers = [
            (1, Rectangle::new((0, 0).into(), (252, 304).into())),
            (2, Rectangle::new((-20, -20).into(), (292, 344).into())),
        ];
        let target_layers = [
            (1, Rectangle::new((0, 0).into(), (1320, 840).into())),
            (2, Rectangle::new((0, 0).into(), (1280, 800).into())),
        ];

        assert!(persistent_endpoint_layers_cover_target(
            (252, 304).into(),
            &outgoing_layers,
            (1280, 800).into(),
            &target_layers,
        ));
    }

    #[test]
    fn x11_target_rejects_a_resized_buffer_with_only_the_old_area_painted() {
        let target = Size::<i32, Buffer>::from((1450, 1252));
        let old_area = Rectangle::new((0, 0).into(), (735, 611).into());

        assert!(!damage_covers_buffer(target, &[old_area]));
    }

    #[test]
    fn x11_target_accepts_tiled_damage_only_after_it_covers_the_buffer() {
        let target = Size::<i32, Buffer>::from((100, 80));
        let mut coverage = TargetDamageCoverage::default();

        assert!(!coverage.observe(
            Some(target),
            &[Rectangle::new((0, 0).into(), (50, 80).into())]
        ));
        assert!(coverage.observe(
            Some(target),
            &[Rectangle::new((50, 0).into(), (50, 80).into())]
        ));
    }

    #[test]
    fn x11_target_discards_damage_when_the_buffer_size_changes() {
        let mut coverage = TargetDamageCoverage::default();

        assert!(coverage.observe(
            Some((100, 80).into()),
            &[Rectangle::new((0, 0).into(), (100, 80).into())]
        ));
        assert!(!coverage.observe(
            Some((200, 160).into()),
            &[Rectangle::new((0, 0).into(), (100, 80).into())]
        ));
    }

    #[test]
    fn repeated_partial_damage_has_constant_storage_and_never_fills_a_hole() {
        let target = (513, 7).into();
        let mut coverage = TargetDamageCoverage::default();
        let strips = [
            Rectangle::new((0, 0).into(), (256, 7).into()),
            Rectangle::new((257, 0).into(), (256, 7).into()),
        ];
        for _ in 0..10_000 {
            assert!(!coverage.observe(Some(target), &strips));
            assert_eq!(coverage.painted.len(), (513_usize * 7).div_ceil(64));
            assert_eq!(coverage.painted_pixels, 512 * 7);
        }
        assert!(coverage.observe(
            Some(target),
            &[Rectangle::new((256, 0).into(), (1, 7).into())]
        ));
        assert!(
            coverage.painted.is_empty(),
            "completed tracking must release storage"
        );
        assert_eq!(coverage.painted.capacity(), 0);
        assert!(coverage.observe(Some(target), &[]));
    }

    #[test]
    fn clipped_overlapping_damage_matches_an_independent_pixel_oracle() {
        const W: usize = 73;
        const H: usize = 9;
        let size = Some((W as i32, H as i32).into());
        let mut coverage = TargetDamageCoverage::default();
        let mut painted = [false; W * H];
        let mut random = 17_u32;
        for _ in 0..500 {
            let mut next = || {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                random
            };
            let x = (next() % 100) as i32 - 15;
            let y = (next() % 20) as i32 - 5;
            let w = (next() % 25) as i32;
            let h = (next() % 8) as i32;
            for row in 0..H {
                for col in 0..W {
                    painted[row * W + col] |= col as i32 >= x
                        && (col as i32) < x + w
                        && row as i32 >= y
                        && (row as i32) < y + h;
                }
            }
            assert_eq!(
                coverage.observe(size, &[Rectangle::new((x, y).into(), (w, h).into())]),
                painted.iter().all(|pixel| *pixel)
            );
            assert_eq!(
                coverage.painted_pixels,
                painted.iter().filter(|pixel| **pixel).count()
            );
        }
        // Fill remaining holes individually, exercising the bitmap completion
        // path rather than the full-buffer fast path.
        for index in 0..painted.len() {
            if !painted[index] {
                painted[index] = true;
                assert_eq!(
                    coverage.observe(
                        size,
                        &[Rectangle::new(
                            ((index % W) as i32, (index / W) as i32).into(),
                            (1, 1).into(),
                        )]
                    ),
                    painted.iter().all(|pixel| *pixel)
                );
            }
        }
        assert!(coverage.observe(size, &[]));
        assert_eq!(coverage.painted.capacity(), 0);
    }

    #[test]
    fn full_repaint_needs_no_bitmap_and_size_changes_release_old_evidence() {
        let mut coverage = TargetDamageCoverage::default();
        let size = (100, 80).into();
        assert!(coverage.observe(Some(size), &[Rectangle::from_size(size)]));
        assert!(coverage.painted.is_empty());
        assert!(coverage.observe(Some(size), &[]));

        assert!(!coverage.observe(Some((200, 160).into()), &[Rectangle::from_size(size)]));
        assert!(!coverage.painted.is_empty());
        for invalid in [None, Some((0, 80).into()), Some((80, 0).into())] {
            assert!(!coverage.observe(invalid, &[]));
            assert!(coverage.painted.is_empty());
            assert_eq!(coverage.painted.capacity(), 0);
            assert_eq!(coverage.painted_pixels, 0);
        }
    }

    #[test]
    fn unallocatable_coverage_fails_closed_but_can_accept_a_later_full_repaint() {
        // This exceeds the address space; no huge allocation is populated.
        if usize::BITS < 64 {
            return;
        }
        let mut coverage = TargetDamageCoverage::default();
        let size = (i32::MAX, i32::MAX).into();
        assert!(!coverage.observe(Some(size), &[Rectangle::from_size((1, 1).into())]));
        assert!(coverage.allocation_failed);
        assert!(coverage.painted.is_empty());
        assert!(!coverage.observe(Some(size), &[Rectangle::from_size((2, 2).into())]));
        assert!(coverage.observe(Some(size), &[Rectangle::from_size(size)]));
        assert!(coverage.painted.is_empty());
        assert!(coverage.observe(Some((8, 8).into()), &[Rectangle::from_size((8, 8).into())]));
        assert!(!coverage.allocation_failed);
    }
}
