use smithay::delegate_background_effect;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::background_effect::{
    self, BackgroundEffectSurfaceCachedState, ExtBackgroundEffectHandler,
};
use smithay::wayland::compositor::{RegionAttributes, with_states};

use crate::compositor::root::Halley;

pub(crate) fn surface_wants_background_blur(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        if !states
            .cached_state
            .has::<BackgroundEffectSurfaceCachedState>()
        {
            return false;
        }
        let mut cached = states
            .cached_state
            .get::<BackgroundEffectSurfaceCachedState>();
        cached.current().blur_region.is_some()
    })
}

impl ExtBackgroundEffectHandler for Halley {
    fn capabilities(&self) -> background_effect::Capability {
        background_effect::Capability::Blur
    }

    fn set_blur_region(&mut self, _wl_surface: WlSurface, _region: RegionAttributes) {
        self.runtime.tty_redraw_all = true;
        self.request_maintenance();
    }

    fn unset_blur_region(&mut self, _wl_surface: WlSurface) {
        self.runtime.tty_redraw_all = true;
        self.request_maintenance();
    }
}

delegate_background_effect!(Halley);
