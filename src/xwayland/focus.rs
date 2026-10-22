use std::borrow::Cow;

use smithay::backend::input::{InputTime, KeyState};
use smithay::desktop::{PopupKind, Window};
use smithay::input::Seat;
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Serial};
use smithay::wayland::seat::WaylandFocus;
#[cfg(feature = "xwayland")]
use smithay::xwayland::X11Surface;

use crate::input::keyboard_delivery::{self, PendingModifiers};
use crate::session::{Session, SessionDriver};

#[derive(Clone, Debug, PartialEq)]
pub enum KeyboardFocusTarget {
    Wayland(WlSurface),
    #[cfg(feature = "xwayland")]
    X11(Box<X11Surface>),
}

impl KeyboardFocusTarget {
    pub fn for_window(window: &Window) -> Option<Self> {
        #[cfg(feature = "xwayland")]
        if let Some(surface) = window.x11_surface() {
            return Some(Self::X11(Box::new(surface.clone())));
        }
        window
            .wl_surface()
            .map(|surface| Self::Wayland(surface.into_owned()))
    }

    pub fn x11_window_id(&self) -> Option<u32> {
        match self {
            Self::Wayland(_) => None,
            #[cfg(feature = "xwayland")]
            Self::X11(surface) => Some(surface.window_id()),
        }
    }

    pub fn globally_active_x11_window_id(&self) -> Option<u32> {
        match self {
            Self::Wayland(_) => None,
            #[cfg(feature = "xwayland")]
            Self::X11(surface)
                if surface.input_model()
                    == smithay::xwayland::xwm::WmInputModel::GloballyActive =>
            {
                Some(surface.window_id())
            }
            #[cfg(feature = "xwayland")]
            Self::X11(_) => None,
        }
    }

    pub fn acknowledge_attention(&self) {
        #[cfg(not(feature = "xwayland"))]
        let _ = self;
        #[cfg(feature = "xwayland")]
        {
            let Self::X11(surface) = self else {
                return;
            };
            if surface.demands_attention()
                && let Err(err) = surface.set_demands_attention(false)
            {
                eventline::warn!(
                    "xwayland: failed to acknowledge attention xid={}: {err}",
                    surface.window_id()
                );
            }
        }
    }

    fn target<D: SessionDriver>(&self) -> &dyn KeyboardTarget<Session<D>> {
        match self {
            Self::Wayland(surface) => surface,
            #[cfg(feature = "xwayland")]
            Self::X11(surface) => surface.as_ref(),
        }
    }

    pub(crate) fn send_modifiers<D: SessionDriver>(
        &self,
        seat: &Seat<Session<D>>,
        session: &mut Session<D>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        self.target().modifiers(seat, session, modifiers, serial);
    }
}

impl From<WlSurface> for KeyboardFocusTarget {
    fn from(surface: WlSurface) -> Self {
        Self::Wayland(surface)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(popup: PopupKind) -> Self {
        Self::Wayland(popup.wl_surface().clone())
    }
}

impl From<KeyboardFocusTarget> for WlSurface {
    fn from(target: KeyboardFocusTarget) -> Self {
        target
            .wl_surface()
            .expect("keyboard focus target has no Wayland surface")
            .into_owned()
    }
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            #[cfg(feature = "xwayland")]
            Self::X11(surface) => surface.alive(),
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            #[cfg(feature = "xwayland")]
            Self::X11(surface) => surface.wl_surface().map(Cow::Owned),
        }
    }
}

impl<D: SessionDriver> KeyboardTarget<Session<D>> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<Session<D>>,
        session: &mut Session<D>,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        keyboard_delivery::flush(session);
        self.target().enter(seat, session, keys, serial);
    }

    fn leave(&self, seat: &Seat<Session<D>>, session: &mut Session<D>, serial: Serial) {
        keyboard_delivery::flush(session);
        self.target().leave(seat, session, serial);
    }

    fn key(
        &self,
        seat: &Seat<Session<D>>,
        session: &mut Session<D>,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        // The pinned Smithay input path calls modifiers before key. Match the
        // same target and serial so only this key's update follows it; other
        // modifier changes must be visible before the key instead.
        let after = session.keyboard.delivery.take().and_then(|pending| {
            if pending.target == *self && pending.serial == serial {
                Some(pending)
            } else {
                pending.send(session);
                None
            }
        });
        self.target().key(seat, session, key, state, serial, time);
        if let Some(pending) = after {
            pending.send(session);
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Session<D>>,
        session: &mut Session<D>,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        if session.keyboard.delivery.active() {
            keyboard_delivery::flush(session);
            session.keyboard.delivery.defer(PendingModifiers {
                target: self.clone(),
                state: modifiers,
                serial,
            });
        } else {
            self.send_modifiers(seat, session, modifiers, serial);
        }
    }
}
