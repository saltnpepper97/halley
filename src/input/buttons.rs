//! Linux evdev pointer button codes, named as in `linux/input-event-codes.h`.
//!
//! Smithay reports buttons as raw evdev codes; these names keep `0x110` and
//! friends out of the input code.

pub(crate) const BTN_LEFT: u32 = 0x110;
pub(crate) const BTN_RIGHT: u32 = 0x111;
pub(crate) const BTN_MIDDLE: u32 = 0x112;
pub(crate) const BTN_SIDE: u32 = 0x113;
pub(crate) const BTN_EXTRA: u32 = 0x114;
pub(crate) const BTN_FORWARD: u32 = 0x115;
pub(crate) const BTN_BACK: u32 = 0x116;
