use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{ContextId, ImportMem, Renderer, Texture};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Logical, Physical, Rectangle, Scale, Size, Transform};
use std::collections::HashMap;
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};

const SQUARE: &str = include_str!("shaders/node_square.frag");
const SQUIRCLE: &str = include_str!("shaders/node_squircle.frag");
const LABEL_SQUARE: &str = include_str!("shaders/ui_rect_square.frag");
const LABEL_ROUNDED: &str = include_str!("shaders/ui_rect_rounded.frag");
const FOCUS_RING: &str = include_str!("shaders/focus_ring.frag");
const DASHED_OUTLINE: &str = include_str!("shaders/dashed_outline.frag");

/// Logical slots owned by [`NodeRenderer`] that need a stable render-element
/// identity. See [`crate::render::ids`] for why this matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeSlot {
    NodeBody(u32),
    NodeLabel(u32),
    OverlayCard(u32),
    AppIcon(u32),
    SelectionCheck(u32),
    SessionLockBackdrop,
    ClusterExclusiveBackdrop,
    ApogeeBackdrop,
    ClusterComposerBackdrop,
    FocusCycleBackdrop,
    HoverPreviewFallback,
    ShellBackdrop,
    SourceChooserBackdrop,
    ClusterCreationBackdrop,
    ClusterNameSelection,
    ClusterNameCaret,
    FocusRing,
    /// One of the four dimmed regions around the capture selection.
    PickerDim(u8),
    PickerOutline,
    /// One of the four corner grips, indexed clockwise from the top left.
    PickerHandle(u8),
    PickerBackdrop,
}

/// Dash count and dot size, preserved from the original quad-per-dot ring so
/// the rendered rhythm stays recognisable.
const FOCUS_RING_SEGMENTS: f32 = 160.0;
const FOCUS_RING_THICKNESS: f32 = 3.0;

/// Ramanujan's approximation, well under a pixel of error for screen-sized
/// ellipses. Used to space dashes evenly by arc length.
fn ellipse_perimeter(rx: f32, ry: f32) -> f32 {
    let h = ((rx - ry) * (rx - ry)) / ((rx + ry) * (rx + ry)).max(f32::EPSILON);
    std::f32::consts::PI * (rx + ry) * (1.0 + (3.0 * h) / (10.0 + (4.0 - 3.0 * h).sqrt()))
}

fn overlay_card_metrics(
    size: Size<i32, Physical>,
    requested_content_radius: f32,
    requested_border_px: f32,
) -> super::window_decoration::Metrics {
    let border_px = requested_border_px
        .max(0.0)
        .min(size.w.min(size.h).max(0) as f32 * 0.5);
    let max_content_radius = (size.w.min(size.h).max(0) as f32 * 0.5 - border_px).max(0.0);
    super::window_decoration::metrics(
        requested_content_radius.max(0.0).min(max_content_radius),
        border_px,
    )
}

fn finish_commit(hasher: DefaultHasher) -> CommitCounter {
    CommitCounter::from(hasher.finish() as usize)
}

fn hash_floats(values: impl IntoIterator<Item = f32>, hasher: &mut DefaultHasher) {
    for value in values {
        value.to_bits().hash(hasher);
    }
}

fn shape_tag(shape: halley_config::NodeShape) -> u8 {
    match shape {
        halley_config::NodeShape::Square => 0,
        halley_config::NodeShape::Squircle => 1,
    }
}

fn node_commit(shape: halley_config::NodeShape, style: NodeStyle) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    shape_tag(shape).hash(&mut hasher);
    hash_floats(
        [
            style.border_rgb.0,
            style.border_rgb.1,
            style.border_rgb.2,
            style.fill_rgb.0,
            style.fill_rgb.1,
            style.fill_rgb.2,
            style.opacity,
        ],
        &mut hasher,
    );
    style.flat_fill.hash(&mut hasher);
    finish_commit(hasher)
}

#[allow(clippy::too_many_arguments)]
fn label_commit(
    shape: halley_config::NodeShape,
    fill: (f32, f32, f32, f32),
    border: (f32, f32, f32, f32),
    destination: Rectangle<i32, Physical>,
    corner_radius: f32,
    inner_offset: f32,
    inner_corner_radius: f32,
    border_px: f32,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    shape_tag(shape).hash(&mut hasher);
    destination.size.w.hash(&mut hasher);
    destination.size.h.hash(&mut hasher);
    hash_floats(
        [
            fill.0,
            fill.1,
            fill.2,
            fill.3,
            border.0,
            border.1,
            border.2,
            border.3,
            corner_radius,
            inner_offset,
            inner_corner_radius,
            border_px,
        ],
        &mut hasher,
    );
    finish_commit(hasher)
}

fn focus_ring_commit(
    destination: Rectangle<i32, Physical>,
    color: (f32, f32, f32, f32),
    radii: (f32, f32),
    dash_period: f32,
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    destination.size.w.hash(&mut hasher);
    destination.size.h.hash(&mut hasher);
    hash_floats(
        [
            color.0,
            color.1,
            color.2,
            color.3,
            radii.0,
            radii.1,
            dash_period,
        ],
        &mut hasher,
    );
    finish_commit(hasher)
}

fn dashed_outline_commit(
    destination: Rectangle<i32, Physical>,
    color: (f32, f32, f32, f32),
    thickness: f32,
    dash: (f32, f32),
) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    destination.size.w.hash(&mut hasher);
    destination.size.h.hash(&mut hasher);
    hash_floats(
        [
            color.0, color.1, color.2, color.3, thickness, dash.0, dash.1,
        ],
        &mut hasher,
    );
    finish_commit(hasher)
}

fn string_commit(value: &str) -> CommitCounter {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    finish_commit(hasher)
}

struct Resources {
    context: ContextId<GlesTexture>,
    texture: GlesTexture,
    square: GlesTexProgram,
    squircle: GlesTexProgram,
    label_square: GlesTexProgram,
    label_rounded: GlesTexProgram,
    focus_ring: GlesTexProgram,
    dashed_outline: GlesTexProgram,
}

#[derive(Default)]
pub struct NodeRenderer {
    resources: Option<Resources>,
    icons: super::app_icon::AppIconCache,
    selection_check: super::selection_check::SelectionCheckRenderer,
    ids: super::ids::OutputElementIds<NodeSlot>,
    active_output: String,
    occurrences: HashMap<u8, u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayCardStyle {
    pub content_radius: f32,
    pub fill: (f32, f32, f32, f32),
    pub border: (f32, f32, f32, f32),
    pub border_px: f32,
    pub alpha: f32,
}

#[derive(Debug)]
pub struct NodeRenderElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    border: (f32, f32, f32, f32),
    fill: (f32, f32, f32, f32),
    fill_alpha: f32,
    flat_fill: bool,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct LabelRenderElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    fill: (f32, f32, f32, f32),
    border: (f32, f32, f32, f32),
    size: (f32, f32),
    corner_radius: f32,
    inner_offset: f32,
    inner_corner_radius: f32,
    border_px: f32,
    commit: CommitCounter,
}

impl LabelRenderElement {
    pub fn corner_radius(&self) -> f32 {
        self.corner_radius
    }
}

#[derive(Clone, Copy)]
pub struct NodeStyle {
    pub border_rgb: (f32, f32, f32),
    pub fill_rgb: (f32, f32, f32),
    pub opacity: f32,
    pub flat_fill: bool,
}

#[derive(Debug)]
pub struct NodeTextureElement {
    base: TextureRenderElement<GlesTexture>,
    commit: CommitCounter,
}

#[derive(Debug)]
pub struct FocusRingElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    color: (f32, f32, f32, f32),
    size: (f32, f32),
    radii: (f32, f32),
    dash_period: f32,
    commit: CommitCounter,
}

impl Element for FocusRingElement {
    fn id(&self) -> &Id {
        self.base.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.base.transform()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        self.base.alpha()
    }
    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

#[derive(Debug)]
pub struct DashedOutlineElement {
    base: TextureRenderElement<GlesTexture>,
    texture: GlesTexture,
    program: GlesTexProgram,
    color: (f32, f32, f32, f32),
    size: (f32, f32),
    thickness: f32,
    dash_period: f32,
    dash_length: f32,
    commit: CommitCounter,
}

impl Element for DashedOutlineElement {
    fn id(&self) -> &Id {
        self.base.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.base.transform()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        self.base.alpha()
    }
    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for DashedOutlineElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.base.alpha(),
            Some(&self.program),
            &[
                Uniform::new("outline_color", self.color),
                Uniform::new("rect_size", self.size),
                Uniform::new("thickness", self.thickness),
                Uniform::new("dash_period", self.dash_period),
                Uniform::new("dash_length", self.dash_length),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl RenderElement<GlesRenderer> for FocusRingElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.base.alpha(),
            Some(&self.program),
            &[
                Uniform::new("ring_color", self.color),
                Uniform::new("rect_size", self.size),
                Uniform::new("radii", self.radii),
                Uniform::new("thickness", FOCUS_RING_THICKNESS),
                Uniform::new("dash_period", self.dash_period),
                Uniform::new("dash_length", FOCUS_RING_THICKNESS),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl NodeRenderer {
    fn dynamic_id(&mut self, kind: u8) -> Id {
        let occurrence = self.occurrences.entry(kind).or_default();
        let index = *occurrence;
        *occurrence = occurrence.wrapping_add(1);
        let slot = match kind {
            0 => NodeSlot::NodeBody(index),
            1 => NodeSlot::NodeLabel(index),
            2 => NodeSlot::OverlayCard(index),
            3 => NodeSlot::AppIcon(index),
            4 => NodeSlot::SelectionCheck(index),
            _ => unreachable!("unknown node render slot kind"),
        };
        self.ids.for_output(&self.active_output).id(slot)
    }

    pub fn has_pending_icons(&self) -> bool {
        self.icons.has_pending()
    }

    /// Per-frame icon-cache maintenance, independent of whether any icon is
    /// drawn this frame. See [`AppIconCache::poll`].
    ///
    /// [`AppIconCache::poll`]: super::app_icon::AppIconCache::poll
    pub fn poll_icons(&mut self, renderer: &mut GlesRenderer) {
        self.icons.poll(renderer);
    }

    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        shape: halley_config::NodeShape,
        style: NodeStyle,
    ) -> Result<NodeRenderElement, Box<dyn Error>> {
        let id = self.dynamic_id(0);
        self.ensure_resources(renderer)?;
        let resources = self.resources.as_ref().expect("ensured above");
        let program = match shape {
            halley_config::NodeShape::Square => resources.square.clone(),
            halley_config::NodeShape::Squircle => resources.squircle.clone(),
        };
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        let base = TextureRenderElement::from_static_texture(
            id,
            resources.context.clone(),
            destination.loc.to_f64(),
            resources.texture.clone(),
            1,
            Transform::Normal,
            Some(1.0),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        );
        let fill = (style.fill_rgb.0, style.fill_rgb.1, style.fill_rgb.2, 1.0);
        Ok(NodeRenderElement {
            base,
            texture: resources.texture.clone(),
            program,
            border: (
                style.border_rgb.0,
                style.border_rgb.1,
                style.border_rgb.2,
                3.0 / 26.0,
            ),
            fill,
            fill_alpha: style.opacity,
            flat_fill: style.flat_fill,
            commit: node_commit(shape, style),
        })
    }

    pub fn label_element(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        shape: halley_config::NodeShape,
        rgb: (f32, f32, f32),
        alpha: f32,
    ) -> Result<LabelRenderElement, Box<dyn Error>> {
        let id = self.dynamic_id(1);
        self.ensure_resources(renderer)?;
        let resources = self.resources.as_ref().expect("ensured above");
        let program = match shape {
            halley_config::NodeShape::Square => resources.label_square.clone(),
            halley_config::NodeShape::Squircle => resources.label_rounded.clone(),
        };
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        let base = TextureRenderElement::from_static_texture(
            id,
            resources.context.clone(),
            destination.loc.to_f64(),
            resources.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha.clamp(0.0, 1.0)),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        );
        Ok(LabelRenderElement {
            base,
            texture: resources.texture.clone(),
            program,
            fill: (rgb.0, rgb.1, rgb.2, 1.0),
            border: (rgb.0, rgb.1, rgb.2, 1.0),
            size: (destination.size.w as f32, destination.size.h as f32),
            corner_radius: destination.size.h as f32 * 0.32,
            inner_offset: 0.0,
            inner_corner_radius: destination.size.h as f32 * 0.32,
            border_px: 0.0,
            commit: label_commit(
                shape,
                (rgb.0, rgb.1, rgb.2, 1.0),
                (rgb.0, rgb.1, rgb.2, 1.0),
                destination,
                destination.size.h as f32 * 0.32,
                0.0,
                destination.size.h as f32 * 0.32,
                0.0,
            ),
        })
    }

    pub fn overlay_card_element(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        style: OverlayCardStyle,
    ) -> Result<LabelRenderElement, Box<dyn Error>> {
        let id = self.dynamic_id(2);
        self.ensure_resources(renderer)?;
        let resources = self.resources.as_ref().expect("ensured above");
        let metrics = overlay_card_metrics(destination.size, style.content_radius, style.border_px);
        let program = if metrics.content_radius > 0.0 {
            resources.label_rounded.clone()
        } else {
            resources.label_square.clone()
        };
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        let base = TextureRenderElement::from_static_texture(
            id,
            resources.context.clone(),
            destination.loc.to_f64(),
            resources.texture.clone(),
            1,
            Transform::Normal,
            Some(style.alpha.clamp(0.0, 1.0)),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        );
        Ok(LabelRenderElement {
            base,
            texture: resources.texture.clone(),
            program,
            fill: style.fill,
            border: style.border,
            size: (destination.size.w as f32, destination.size.h as f32),
            corner_radius: metrics.outer_radius,
            inner_offset: metrics.inner_offset,
            inner_corner_radius: metrics.inner_radius,
            border_px: metrics.border_width,
            commit: label_commit(
                if metrics.content_radius > 0.0 {
                    halley_config::NodeShape::Squircle
                } else {
                    halley_config::NodeShape::Square
                },
                style.fill,
                style.border,
                destination,
                metrics.outer_radius,
                metrics.inner_offset,
                metrics.inner_radius,
                metrics.border_width,
            ),
        })
    }

    /// One dashed focus ring, drawn by a single shader element.
    ///
    /// This used to be [`FOCUS_RING_SEGMENTS`] individual 3x3 solid quads with
    /// a fresh `Id` each. That cost 160 draw calls and, worse, told the damage
    /// tracker that 160 elements appeared and vanished every frame — which
    /// damaged the whole output and forced a full-screen backdrop-blur
    /// re-capture on every frame the ring was visible.
    pub fn focus_ring_element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &str,
        center: (f32, f32),
        radii: (f32, f32),
        rgb: (f32, f32, f32),
        alpha: f32,
    ) -> Result<FocusRingElement, Box<dyn Error>> {
        self.ensure_resources(renderer)?;
        let id = self.ids.for_output(output).id(NodeSlot::FocusRing);
        let resources = self.resources.as_ref().expect("ensured above");

        let (rx, ry) = (radii.0.max(1.0), radii.1.max(1.0));
        // Pad by the dash thickness so antialiasing at the outer edge is not
        // clipped by the element's own bounds.
        let pad = FOCUS_RING_THICKNESS + 1.0;
        let half_w = rx + pad;
        let half_h = ry + pad;
        let destination = Rectangle::<i32, Physical>::new(
            (
                (center.0 - half_w).round() as i32,
                (center.1 - half_h).round() as i32,
            )
                .into(),
            ((half_w * 2.0).round() as i32, (half_h * 2.0).round() as i32).into(),
        );
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        let base = TextureRenderElement::from_static_texture(
            id,
            resources.context.clone(),
            destination.loc.to_f64(),
            resources.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha.clamp(0.0, 1.0)),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        );
        Ok(FocusRingElement {
            base,
            texture: resources.texture.clone(),
            program: resources.focus_ring.clone(),
            color: (rgb.0, rgb.1, rgb.2, 1.0),
            size: (destination.size.w as f32, destination.size.h as f32),
            radii: (rx, ry),
            dash_period: ellipse_perimeter(rx, ry) / FOCUS_RING_SEGMENTS,
            commit: focus_ring_commit(
                destination,
                (rgb.0, rgb.1, rgb.2, 1.0),
                (rx, ry),
                ellipse_perimeter(rx, ry) / FOCUS_RING_SEGMENTS,
            ),
        })
    }

    pub fn app_icon_element(
        &mut self,
        renderer: &mut GlesRenderer,
        app_id: &str,
        destination: Rectangle<i32, Physical>,
        alpha: f32,
    ) -> Option<NodeTextureElement> {
        let id = self.dynamic_id(3);
        self.icons
            .element(renderer, id, app_id, destination, alpha.clamp(0.0, 1.0))
            .map(|base| NodeTextureElement {
                base,
                commit: string_commit(app_id),
            })
    }

    pub fn request_app_icon(&mut self, renderer: &mut GlesRenderer, app_id: &str) {
        self.icons.request(renderer, app_id);
    }

    pub fn selection_check_element(
        &mut self,
        renderer: &mut GlesRenderer,
        destination: Rectangle<i32, Physical>,
        rgb: [u8; 3],
        alpha: f32,
    ) -> Option<NodeTextureElement> {
        let id = self.dynamic_id(4);
        let base = self
            .selection_check
            .element(renderer, id, destination, rgb, alpha)?;
        let mut hasher = DefaultHasher::new();
        destination.size.w.hash(&mut hasher);
        destination.size.h.hash(&mut hasher);
        rgb.hash(&mut hasher);
        Some(NodeTextureElement {
            base,
            commit: finish_commit(hasher),
        })
    }

    /// Advances this renderer's stable-identity generation for `output`, so
    /// slots that stop being drawn are eventually released.
    pub fn begin_scene(&mut self, output: &str) {
        self.active_output.clear();
        self.active_output.push_str(output);
        self.occurrences.clear();
        self.ids.advance(output);
    }

    /// Stable render-element identity for one of this renderer's slots.
    ///
    /// Exposed so callers that build plain [`SolidColorRenderElement`]s can
    /// still keep a frame-to-frame identity instead of minting a new `Id`.
    ///
    /// [`SolidColorRenderElement`]: smithay::backend::renderer::element::solid::SolidColorRenderElement
    pub fn slot_id(&mut self, output: &str, slot: NodeSlot) -> Id {
        self.ids.for_output(output).id(slot)
    }

    pub fn active_slot_id(&mut self, slot: NodeSlot) -> Id {
        self.ids.for_output(&self.active_output).id(slot)
    }

    /// A dashed (or, with `dash_length >= dash_period`, solid) rectangular
    /// outline drawn inside `destination` by one shader element.
    #[allow(clippy::too_many_arguments)]
    pub fn dashed_outline_element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: &str,
        slot: NodeSlot,
        destination: Rectangle<i32, Physical>,
        rgba: (f32, f32, f32, f32),
        thickness: f32,
        dash: (f32, f32),
    ) -> Result<DashedOutlineElement, Box<dyn Error>> {
        self.ensure_resources(renderer)?;
        let id = self.ids.for_output(output).id(slot);
        let resources = self.resources.as_ref().expect("ensured above");
        let source = Rectangle::<f64, Logical>::new(
            (0.0, 0.0).into(),
            (
                resources.texture.size().w as f64,
                resources.texture.size().h as f64,
            )
                .into(),
        );
        let base = TextureRenderElement::from_static_texture(
            id,
            resources.context.clone(),
            destination.loc.to_f64(),
            resources.texture.clone(),
            1,
            Transform::Normal,
            Some(rgba.3.clamp(0.0, 1.0)),
            Some(source),
            Some(destination.size.to_logical(1)),
            None,
            Kind::Unspecified,
        );
        Ok(DashedOutlineElement {
            base,
            texture: resources.texture.clone(),
            program: resources.dashed_outline.clone(),
            color: (rgba.0, rgba.1, rgba.2, 1.0),
            size: (destination.size.w as f32, destination.size.h as f32),
            thickness,
            dash_period: dash.0,
            dash_length: dash.1,
            commit: dashed_outline_commit(destination, rgba, thickness, dash),
        })
    }

    fn ensure_resources(&mut self, renderer: &mut GlesRenderer) -> Result<(), Box<dyn Error>> {
        let context = renderer.context_id();
        if self
            .resources
            .as_ref()
            .is_some_and(|resources| resources.context == context)
        {
            return Ok(());
        }
        let texture =
            renderer.import_memory(&[255_u8; 4 * 4 * 4], Fourcc::Abgr8888, (4, 4).into(), false)?;
        let uniforms = [
            UniformName::new("node_color", UniformType::_4f),
            UniformName::new("fill_color", UniformType::_4f),
            UniformName::new("flat_fill", UniformType::_1f),
            UniformName::new("center_flat_fill", UniformType::_1f),
            UniformName::new("fill_alpha", UniformType::_1f),
        ];
        let square = renderer.compile_custom_texture_shader(SQUARE, &uniforms)?;
        let squircle = renderer.compile_custom_texture_shader(SQUIRCLE, &uniforms)?;
        let label_uniforms = [
            UniformName::new("node_color", UniformType::_4f),
            UniformName::new("fill_color", UniformType::_4f),
            UniformName::new("rect_size", UniformType::_2f),
            UniformName::new("inner_rect_size", UniformType::_2f),
            UniformName::new("inner_rect_offset", UniformType::_2f),
            UniformName::new("corner_radius", UniformType::_1f),
            UniformName::new("inner_corner_radius", UniformType::_1f),
            UniformName::new("border_px", UniformType::_1f),
        ];
        let label_square = renderer.compile_custom_texture_shader(LABEL_SQUARE, &label_uniforms)?;
        let label_rounded =
            renderer.compile_custom_texture_shader(LABEL_ROUNDED, &label_uniforms)?;
        let focus_ring = renderer.compile_custom_texture_shader(
            FOCUS_RING,
            &[
                UniformName::new("ring_color", UniformType::_4f),
                UniformName::new("rect_size", UniformType::_2f),
                UniformName::new("radii", UniformType::_2f),
                UniformName::new("thickness", UniformType::_1f),
                UniformName::new("dash_period", UniformType::_1f),
                UniformName::new("dash_length", UniformType::_1f),
            ],
        )?;
        let dashed_outline = renderer.compile_custom_texture_shader(
            DASHED_OUTLINE,
            &[
                UniformName::new("outline_color", UniformType::_4f),
                UniformName::new("rect_size", UniformType::_2f),
                UniformName::new("thickness", UniformType::_1f),
                UniformName::new("dash_period", UniformType::_1f),
                UniformName::new("dash_length", UniformType::_1f),
            ],
        )?;
        self.resources = Some(Resources {
            context,
            texture,
            square,
            squircle,
            label_square,
            label_rounded,
            focus_ring,
            dashed_outline,
        });
        Ok(())
    }
}

impl Element for NodeRenderElement {
    fn id(&self) -> &Id {
        self.base.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.base.transform()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        1.0
    }
    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl Element for NodeTextureElement {
    fn id(&self) -> &Id {
        self.base.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.base.transform()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.base.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.base.alpha()
    }
    fn kind(&self) -> Kind {
        self.base.kind()
    }
}

impl RenderElement<GlesRenderer> for NodeTextureElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::draw(
            &self.base,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        <TextureRenderElement<GlesTexture> as RenderElement<GlesRenderer>>::underlying_storage(
            &self.base, renderer,
        )
    }
}

impl Element for LabelRenderElement {
    fn id(&self) -> &Id {
        self.base.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.base.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.base.transform()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.base.src()
    }
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }
    fn alpha(&self) -> f32 {
        self.base.alpha()
    }
    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for LabelRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.base.alpha(),
            Some(&self.program),
            &[
                Uniform::new("node_color", self.border),
                Uniform::new("fill_color", self.fill),
                Uniform::new("rect_size", self.size),
                Uniform::new(
                    "inner_rect_size",
                    (
                        (self.size.0 - self.inner_offset * 2.0).max(1.0),
                        (self.size.1 - self.inner_offset * 2.0).max(1.0),
                    ),
                ),
                Uniform::new("inner_rect_offset", (self.inner_offset, self.inner_offset)),
                Uniform::new("corner_radius", self.corner_radius),
                Uniform::new("inner_corner_radius", self.inner_corner_radius),
                Uniform::new("border_px", self.border_px),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

impl RenderElement<GlesRenderer> for NodeRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
            Some(&self.program),
            &[
                Uniform::new("node_color", self.border),
                Uniform::new("fill_color", self.fill),
                Uniform::new("flat_fill", if self.flat_fill { 1.0_f32 } else { 0.0_f32 }),
                Uniform::new("center_flat_fill", 0.0_f32),
                Uniform::new("fill_alpha", self.fill_alpha),
            ],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_commits_are_stable_but_follow_shader_inputs() {
        let style = NodeStyle {
            border_rgb: (0.8, 0.2, 0.1),
            fill_rgb: (0.1, 0.2, 0.3),
            opacity: 0.9,
            flat_fill: false,
        };
        assert_eq!(
            node_commit(halley_config::NodeShape::Squircle, style),
            node_commit(halley_config::NodeShape::Squircle, style)
        );
        assert_ne!(
            node_commit(halley_config::NodeShape::Squircle, style),
            node_commit(
                halley_config::NodeShape::Squircle,
                NodeStyle {
                    opacity: 0.5,
                    ..style
                }
            )
        );
        assert_ne!(
            node_commit(halley_config::NodeShape::Squircle, style),
            node_commit(halley_config::NodeShape::Square, style)
        );
    }

    #[test]
    fn animated_outline_commits_follow_geometry_and_phase_inputs() {
        let rect = Rectangle::new((0, 0).into(), (300, 180).into());
        let color = (0.8, 0.4, 0.2, 1.0);
        assert_eq!(
            dashed_outline_commit(rect, color, 2.0, (16.0, 10.0)),
            dashed_outline_commit(rect, color, 2.0, (16.0, 10.0))
        );
        assert_ne!(
            dashed_outline_commit(rect, color, 2.0, (16.0, 10.0)),
            dashed_outline_commit(rect, color, 3.0, (16.0, 10.0))
        );
    }
}
