#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub super_key: bool,
    pub left_super: bool,
    pub right_super: bool,
    pub alt: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub ctrl: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub shift: bool,
    pub left_shift: bool,
    pub right_shift: bool,
}

#[derive(Clone, Debug)]
pub struct Keybinds {
    pub modifier: KeyModifiers,
}

#[derive(Clone, Debug)]
pub struct LaunchBinding {
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub command: String,
}

pub const WHEEL_UP_CODE: u32 = 0x2000;
pub const WHEEL_DOWN_CODE: u32 = 0x2001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionalAction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeBindingAction {
    Move(DirectionalAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrailBindingAction {
    Prev,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusCycleBindingAction {
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorBindingTarget {
    Direction(DirectionalAction),
    Output(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorBindingAction {
    Focus(MonitorBindingTarget),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackCycleDirection {
    Forward,
    Backward,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackBindingAction {
    Cycle(StackCycleDirection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileBindingAction {
    Focus(DirectionalAction),
    Swap(DirectionalAction),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterBindingAction {
    LayoutCycle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositorBindingScope {
    Global,
    Field,
    Cluster,
    Tile,
    Stack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BearingsBindingAction {
    Show,
    Toggle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositorBindingAction {
    Reload,
    OpenTerminal,
    ToggleState,
    CloseFocusedWindow,
    ClusterMode,
    FocusCycle(FocusCycleBindingAction),
    Quit { requires_shift: bool },
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Node(NodeBindingAction),
    Trail(TrailBindingAction),
    Monitor(MonitorBindingAction),
    Bearings(BearingsBindingAction),
    Stack(StackBindingAction),
    Tile(TileBindingAction),
    Cluster(ClusterBindingAction),
}

#[derive(Clone, Debug)]
pub struct CompositorBinding {
    pub scope: CompositorBindingScope,
    pub modifiers: KeyModifiers,
    pub key: u32,
    pub action: CompositorBindingAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerBindingAction {
    MoveWindow,
    FieldJump,
    ResizeWindow,
}

#[derive(Clone, Debug)]
pub struct PointerBinding {
    pub modifiers: KeyModifiers,
    pub button: u32,
    pub action: PointerBindingAction,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            modifier: KeyModifiers {
                left_alt: true,
                ..KeyModifiers::default()
            },
        }
    }
}

impl Keybinds {
    pub fn modifier_name(&self) -> String {
        let mut parts = Vec::new();

        if self.modifier.left_super {
            parts.push("lsuper");
        }
        if self.modifier.right_super {
            parts.push("rsuper");
        }
        if self.modifier.super_key {
            parts.push("super");
        }

        if self.modifier.left_ctrl {
            parts.push("lctrl");
        }
        if self.modifier.right_ctrl {
            parts.push("rctrl");
        }
        if self.modifier.ctrl {
            parts.push("ctrl");
        }

        if self.modifier.left_alt {
            parts.push("lalt");
        }
        if self.modifier.right_alt {
            parts.push("ralt");
        }
        if self.modifier.alt {
            parts.push("alt");
        }

        if self.modifier.left_shift {
            parts.push("lshift");
        }
        if self.modifier.right_shift {
            parts.push("rshift");
        }
        if self.modifier.shift {
            parts.push("shift");
        }

        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}
