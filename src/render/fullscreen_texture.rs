use std::collections::HashMap;
use std::error::Error;

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

use super::window_texture::ResizeWindowTexture;

#[derive(Debug)]
struct TransitionTextures {
    id: Id,
    previous: ResizeWindowTexture,
    /// Incoming endpoint, refreshed offscreen each animation frame. The live
    /// tree stays out of the scene; clip-reveal hides unpainted extra area of a
    /// newly-allocated buffer (the black Firefox fullscreen allocation) until
    /// later commits paint it into this snapshot.
    next: Option<ResizeWindowTexture>,
    /// Scratch endpoint used for validation. It becomes `next` only if the
    /// owning transaction accepts the same commit. Mixed Firefox surface-tree
    /// commits never overwrite the displayed endpoint before that.
    prepared: Option<ResizeWindowTexture>,
    owner: TextureTransitionOwner,
    last_surface_commit: CommitCounter,
    outgoing_surface_size: Option<Size<i32, Logical>>,
    requires_fresh_damage: bool,
    target_readiness: TargetReadiness,
    target_damage: TargetDamageCoverage,
    capture_generation: CommitCounter,
}

#[derive(Debug, Default)]
struct TargetDamageCoverage {
    size: Option<Size<i32, Buffer>>,
    damage: Vec<Rectangle<i32, Buffer>>,
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
            self.size = size;
            self.damage.clear();
        }
        let Some(size) = size.filter(|size| size.w > 0 && size.h > 0) else {
            return false;
        };
        self.damage.extend_from_slice(damage);
        damage_covers_buffer(size, &self.damage)
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
                owner,
                last_surface_commit,
                outgoing_surface_size,
                requires_fresh_damage: crate::xwayland::is_x11(window),
                target_readiness: TargetReadiness::default(),
                target_damage: TargetDamageCoverage::default(),
                capture_generation: CommitCounter::default(),
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
        let candidate = super::window_texture::capture_for_resize(renderer, window, None)?;
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
    ) -> Result<Option<super::resize::ResizeRenderElement>, Box<dyn Error>> {
        let BlendRequest {
            window,
            destination,
            progress,
            hold_previous_until_restored_buffer_matches,
            alpha,
            radii,
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
            let reusable = entry.prepared.take().map(|candidate| candidate.texture);
            let candidate =
                match super::window_texture::capture_for_resize(renderer, window, reusable) {
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
            let next = entry
                .next
                .clone()
                .expect("authorized resize transition has an accepted endpoint");
            let texture_progress = if expanding_endpoint_is_painted(&entry.previous, &next) {
                progress
            } else {
                0.0
            };
            (next, texture_progress)
        } else {
            (
                ResizeWindowTexture {
                    texture: entry.previous.texture.clone(),
                    context: entry.previous.context.clone(),
                    surface_geometry: entry.previous.surface_geometry,
                    window_size: entry.previous.window_size,
                    client_opaque: entry.previous.client_opaque,
                    opaque_area: entry.previous.opaque_area,
                    surface_layers: entry.previous.surface_layers.clone(),
                },
                0.0,
            )
        };
        let id = entry.id.clone();
        Ok(Some(self.resize_renderer.element(
            renderer,
            id,
            &entry.previous,
            next,
            destination,
            texture_progress as f32,
            alpha,
            radii,
            entry.capture_generation,
        )?))
    }
}

pub(crate) fn expanding_endpoint_is_painted(
    outgoing: &ResizeWindowTexture,
    incoming: &ResizeWindowTexture,
) -> bool {
    incoming_paint_is_ready(
        outgoing.window_size,
        outgoing.opaque_area,
        incoming.window_size,
        incoming.client_opaque,
        incoming.opaque_area,
    )
}

fn incoming_paint_is_ready(
    outgoing_size: Size<i32, Physical>,
    outgoing_opaque_area: i64,
    incoming_size: Size<i32, Physical>,
    incoming_opaque: bool,
    incoming_opaque_area: i64,
) -> bool {
    let expanding = incoming_size.w > outgoing_size.w || incoming_size.h > outgoing_size.h;
    if !expanding {
        return true;
    }
    // Firefox can attach a larger unpainted allocation whose opaque area is
    // still empty. Hold the outgoing snapshot then. Clients that never
    // advertise opaque regions (Quickshell / DMS Settings) report 0 on both
    // ends even after they have finished painting; requiring `> 0` froze
    // their enter blend at the windowed frame for the whole motion. Exit
    // already skipped this hold because the destination shrinks.
    incoming_opaque || incoming_opaque_area >= outgoing_opaque_area
}

pub(crate) fn snapshot_matches_target_endpoint(
    outgoing: &ResizeWindowTexture,
    candidate: &ResizeWindowTexture,
) -> bool {
    surface_tree_matches_target_endpoint(
        outgoing.window_size,
        candidate.surface_geometry,
        candidate.window_size,
    ) && persistent_endpoint_layers_cover_target(
        outgoing.window_size,
        &outgoing.surface_layers,
        candidate.window_size,
        &candidate.surface_layers,
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

fn damage_covers_buffer(size: Size<i32, Buffer>, damage: &[Rectangle<i32, Buffer>]) -> bool {
    super::window_texture::rectangles_cover_size(size, damage)
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
    use super::{
        TargetDamageCoverage, TargetReadiness, commit_has_advanced, damage_covers_buffer,
        incoming_paint_is_ready, native_target_buffer_ready, native_target_candidate_ready,
        persistent_endpoint_layers_cover_target, surface_tree_matches_target_endpoint,
        transition_buffer_ready,
    };
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Buffer, Logical, Physical, Rectangle, Size};

    #[test]
    fn expanding_firefox_buffer_does_not_blend_until_it_has_paint() {
        let windowed = Size::<i32, Physical>::from((800, 600));
        let fullscreen = Size::<i32, Physical>::from((1920, 1080));
        let painted = i64::from(800 * 600);

        assert!(!incoming_paint_is_ready(
            windowed, painted, fullscreen, false, 0
        ));
        assert!(incoming_paint_is_ready(
            windowed, painted, fullscreen, true, painted
        ));
        assert!(incoming_paint_is_ready(
            windowed, painted, fullscreen, false, painted
        ));
        assert!(incoming_paint_is_ready(
            fullscreen, painted, windowed, false, 0
        ));
    }

    #[test]
    fn translucent_expanding_window_blends_without_opaque_regions() {
        let windowed = Size::<i32, Physical>::from((1155, 910));
        let maximized = Size::<i32, Physical>::from((1920, 1200));

        assert!(incoming_paint_is_ready(windowed, 0, maximized, false, 0));
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
}
