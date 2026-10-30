use std::collections::{HashMap, HashSet};
use std::error::Error;

use halley_core::field::NodeId;
use smithay::backend::renderer::Texture;
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::desktop::Window;
use smithay::utils::{Physical, Rectangle};

use crate::render::window_texture::WindowTexture;

struct Entry {
    id: Id,
    texture: WindowTexture,
    maximized: bool,
}

/// GPU-local overview previews shared by Apogee and the focus cycle.
///
/// Captures render directly into GLES textures, so DMA-BUF-backed client
/// buffers remain on the GPU. Commits only mark an entry dirty; the texture is
/// refreshed when an overlay actually presents it.
#[derive(Default)]
pub struct OverlayPreviewCache {
    entries: HashMap<NodeId, Entry>,
    dirty: HashSet<NodeId>,
}

pub struct OverlayPreviewRequest<'a> {
    pub id: NodeId,
    pub window: &'a Window,
    pub destination: Rectangle<i32, Physical>,
    pub alpha: f32,
    /// Whether the client is still mapped and can safely be sampled again.
    /// Collapsed nodes deliberately keep using their last-good snapshot.
    pub allow_refresh: bool,
    pub live: bool,
    pub decorations: &'a halley_config::Decorations,
    pub font: &'a halley_config::Font,
    pub chrome_visible: bool,
    pub maximized: bool,
}

pub struct OverlayPreviewRenderers<'a> {
    pub titlebar: &'a mut crate::render::titlebar::TitlebarRenderer,
    pub decoration: &'a mut crate::render::window_decoration::WindowDecorationRenderer,
    pub node: &'a mut crate::render::node::NodeRenderer,
    pub text: &'a mut crate::render::text::UiTextRenderer,
}

fn needs_refresh(
    has_entry: bool,
    allow_refresh: bool,
    maximized_changed: bool,
    live_dirty: bool,
) -> bool {
    !has_entry || allow_refresh && (maximized_changed || live_dirty)
}

impl OverlayPreviewCache {
    /// Reuse an already-current GPU snapshot for compositor-driven handoffs.
    ///
    /// A dirty entry predates the client's latest commit and must not replace
    /// the live window during a close/collapse animation.
    pub fn clean_texture(&self, id: NodeId, maximized: bool) -> Option<WindowTexture> {
        (!self.dirty.contains(&id))
            .then(|| self.entries.get(&id))
            .flatten()
            .filter(|entry| entry.maximized == maximized)
            .map(|entry| entry.texture.clone())
    }

    /// Preserve a compositor-owned last-good frame for a window that is about
    /// to leave the mapped scene. Collapsed nodes keep their client surface
    /// hidden, so trying to create their first preview later can otherwise
    /// sample an empty surface tree (Steam commonly exposes this as a white
    /// Alt-Tab/Apogee tile).
    pub fn store_last_good(&mut self, id: NodeId, texture: WindowTexture, maximized: bool) {
        // The texture contents are new, so the render-element identity must
        // be new as well. Reusing the old Id makes output damage tracking treat
        // the replacement as unchanged and only repaint later hover rectangles.
        self.entries.insert(
            id,
            Entry {
                id: Id::new(),
                texture,
                maximized,
            },
        );
        self.dirty.remove(&id);
    }

    /// Returns the captured texture dimensions without exposing renderer cache
    /// internals to overlay layout code.
    pub fn source_dimensions(&self, id: NodeId) -> Option<(i32, i32)> {
        self.entries.get(&id).map(|entry| {
            let size = entry.texture.texture.size();
            (size.w, size.h)
        })
    }

    pub fn mark_dirty(&mut self, id: NodeId) {
        self.dirty.insert(id);
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty.extend(self.entries.keys().copied());
    }

    pub fn remove(&mut self, id: NodeId) {
        self.entries.remove(&id);
        self.dirty.remove(&id);
    }

    pub fn retain(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        let ids = ids.into_iter().collect::<HashSet<_>>();
        self.entries.retain(|id, _| ids.contains(id));
        self.dirty.retain(|id| ids.contains(id));
    }

    pub fn element_with_texture(
        &mut self,
        renderer: &mut GlesRenderer,
        request: OverlayPreviewRequest<'_>,
        renderers: OverlayPreviewRenderers<'_>,
    ) -> Result<(TextureRenderElement<GlesTexture>, GlesTexture), Box<dyn Error>> {
        let OverlayPreviewRequest {
            id,
            window,
            destination,
            alpha,
            allow_refresh,
            live,
            decorations,
            font,
            chrome_visible,
            maximized,
        } = request;
        let existing = self.entries.get(&id);
        let refresh = needs_refresh(
            existing.is_some(),
            allow_refresh,
            existing.is_some_and(|entry| entry.maximized != maximized),
            live && self.dirty.contains(&id),
        );
        if refresh {
            let previous = self.entries.remove(&id);
            let reusable = previous.as_ref().map(|entry| entry.texture.texture.clone());
            match crate::render::window_texture::capture_decorated(
                renderer,
                window,
                reusable,
                decorations,
                font,
                false,
                chrome_visible,
                maximized,
                true,
                renderers.titlebar,
                renderers.decoration,
                renderers.node,
                renderers.text,
            ) {
                Ok(texture) => {
                    self.dirty.remove(&id);
                    self.entries.insert(
                        id,
                        Entry {
                            // A refreshed texture is new damage even when its
                            // allocation was reused in place.
                            id: Id::new(),
                            texture,
                            maximized,
                        },
                    );
                }
                Err(err) => {
                    if let Some(previous) = previous {
                        self.entries.insert(id, previous);
                    }
                    self.dirty.insert(id);
                    return Err(err);
                }
            }
        }
        let entry = self
            .entries
            .get(&id)
            .ok_or("overview preview capture did not produce a texture")?;
        Ok((
            entry
                .texture
                .render_element(entry.id.clone(), destination, alpha),
            entry.texture.texture.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::needs_refresh;

    #[test]
    fn collapsed_previews_keep_the_last_good_texture() {
        assert!(!needs_refresh(true, false, true, true));
        assert!(!needs_refresh(true, false, false, true));
    }

    #[test]
    fn mapped_previews_refresh_only_for_real_changes() {
        assert!(!needs_refresh(true, true, false, false));
        assert!(needs_refresh(true, true, true, false));
        assert!(needs_refresh(true, true, false, true));
        assert!(needs_refresh(false, false, false, false));
    }
}
