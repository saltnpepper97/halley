//! Order a key's modifier update after that key without delaying standalone
//! modifier updates, focus changes, or intercepted keyboard events.
use smithay::input::keyboard::ModifiersState;
use smithay::utils::Serial;

use crate::session::{Session, SessionDriver};
use crate::xwayland::KeyboardFocusTarget;

pub(crate) struct PendingModifiers {
    pub target: KeyboardFocusTarget,
    pub state: ModifiersState,
    pub serial: Serial,
}

impl PendingModifiers {
    pub fn send<D: SessionDriver>(self, session: &mut Session<D>) {
        let seat = session.seat.clone();
        self.target
            .send_modifiers(&seat, session, self.state, self.serial);
    }
}

#[derive(Default)]
pub(crate) struct Delivery {
    depth: usize,
    pending: Option<PendingModifiers>,
}

impl Delivery {
    pub fn begin(&mut self) {
        self.depth += 1;
    }

    pub fn active(&self) -> bool {
        self.depth > 0
    }

    pub fn take(&mut self) -> Option<PendingModifiers> {
        self.pending.take()
    }

    pub fn defer(&mut self, pending: PendingModifiers) {
        self.pending = Some(pending);
    }
}

pub(crate) fn flush<D: SessionDriver>(session: &mut Session<D>) {
    if let Some(pending) = session.keyboard.delivery.take() {
        pending.send(session);
    }
}

pub(crate) fn finish<D: SessionDriver>(session: &mut Session<D>) {
    session.keyboard.delivery.depth -= 1;
    if !session.keyboard.delivery.active() {
        flush(session);
    }
}
