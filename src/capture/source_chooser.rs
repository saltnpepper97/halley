use smithay::utils::{Logical, Point, Rectangle};

const BAR_WIDTH: i32 = 360;
const BAR_HEIGHT: i32 = 80;
const BAR_BOTTOM_MARGIN: i32 = 28;
const ITEM_PADDING: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMode {
    Monitor,
    Window,
}

impl SourceMode {
    pub const ALL: [Self; 2] = [Self::Monitor, Self::Window];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePhase {
    Menu,
    MonitorPick,
    WindowPick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMenuLayout {
    pub bar: Rectangle<i32, Logical>,
    pub items: [Rectangle<i32, Logical>; 2],
}

#[derive(Debug, Default)]
pub struct SourceChooser {
    active: bool,
    source_types: u32,
    has_category_menu: bool,
    phase: Option<SourcePhase>,
    output_name: String,
    output_geometry: Rectangle<i32, Logical>,
    selected: usize,
    hovered: Option<usize>,
    source: Option<halley_ipc::CaptureSource>,
    source_geometry: Option<Rectangle<i32, Logical>>,
}

impl SourceChooser {
    pub fn begin(
        &mut self,
        source_types: u32,
        output_name: String,
        output_geometry: Rectangle<i32, Logical>,
    ) {
        self.active = true;
        self.source_types = source_types;
        let monitor = source_types & halley_ipc::SOURCE_MONITOR != 0;
        let window = source_types & halley_ipc::SOURCE_WINDOW != 0;
        self.has_category_menu = monitor && window;
        self.phase = Some(match (monitor, window) {
            (true, false) => SourcePhase::MonitorPick,
            (false, true) => SourcePhase::WindowPick,
            _ => SourcePhase::Menu,
        });
        self.output_name = output_name;
        self.output_geometry = output_geometry;
        self.selected = if self.is_enabled(0) { 0 } else { 1 };
        self.hovered = Some(self.selected);
        if self.phase == Some(SourcePhase::MonitorPick) {
            self.source = Some(monitor_source(&self.output_name, self.output_geometry));
            self.source_geometry = Some(self.output_geometry);
        } else {
            self.source = None;
            self.source_geometry = None;
        }
    }

    pub fn cancel(&mut self) {
        *self = Self::default();
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn phase(&self) -> Option<SourcePhase> {
        self.phase
    }

    pub fn output_name(&self) -> &str {
        &self.output_name
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    pub fn monitor_available(&self) -> bool {
        self.source_types & halley_ipc::SOURCE_MONITOR != 0
    }

    pub fn window_available(&self) -> bool {
        self.source_types & halley_ipc::SOURCE_WINDOW != 0
    }

    pub fn selection_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        self.source_geometry
    }

    pub fn update_output_geometry(&mut self, geometry: Rectangle<i32, Logical>) {
        self.output_geometry = geometry;
        if self.phase == Some(SourcePhase::MonitorPick) {
            self.source = Some(monitor_source(&self.output_name, geometry));
            self.source_geometry = Some(geometry);
        }
    }

    pub fn hit_test(&self, position: Point<f64, Logical>) -> Option<usize> {
        let position = position.to_i32_round();
        layout(self.output_geometry)
            .items
            .iter()
            .position(|item| item.contains(position))
            .filter(|index| self.is_enabled(*index))
    }

    pub fn hover_menu(&mut self, position: Point<f64, Logical>) -> bool {
        if self.phase != Some(SourcePhase::Menu) {
            return false;
        }
        let hovered = self.hit_test(position);
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        if let Some(index) = hovered {
            self.selected = index;
        }
        changed
    }

    pub fn move_selection(&mut self, delta: i32) -> bool {
        if self.phase != Some(SourcePhase::Menu) {
            return false;
        }
        let direction = if delta < 0 { -1 } else { 1 };
        let count = SourceMode::ALL.len() as i32;
        let mut candidate = self.selected as i32;
        for _ in 0..count {
            candidate = (candidate + direction).rem_euclid(count);
            if self.is_enabled(candidate as usize) {
                self.selected = candidate as usize;
                self.hovered = Some(self.selected);
                return true;
            }
        }
        false
    }

    pub fn activate_selected(&mut self) -> bool {
        self.activate(SourceMode::ALL[self.selected])
    }

    pub fn activate(&mut self, mode: SourceMode) -> bool {
        if self.phase != Some(SourcePhase::Menu) {
            return false;
        }
        let index = SourceMode::ALL
            .iter()
            .position(|candidate| *candidate == mode)
            .expect("source mode belongs to ALL");
        if !self.is_enabled(index) {
            return false;
        }
        self.phase = Some(match mode {
            SourceMode::Monitor => SourcePhase::MonitorPick,
            SourceMode::Window => SourcePhase::WindowPick,
        });
        if mode == SourceMode::Monitor {
            self.source = Some(monitor_source(&self.output_name, self.output_geometry));
            self.source_geometry = Some(self.output_geometry);
        } else {
            self.source = None;
            self.source_geometry = None;
        }
        true
    }

    pub fn return_to_menu(&mut self) -> bool {
        if !self.active || !self.has_category_menu || self.phase == Some(SourcePhase::Menu) {
            return false;
        }
        self.phase = Some(SourcePhase::Menu);
        self.source = None;
        self.source_geometry = None;
        true
    }

    pub fn hover_source(
        &mut self,
        monitor: halley_ipc::CaptureSource,
        window: Option<(halley_ipc::CaptureSource, Rectangle<i32, Logical>)>,
        monitor_geometry: Rectangle<i32, Logical>,
    ) -> bool {
        let choice = match self.phase {
            Some(SourcePhase::MonitorPick) => Some((monitor, monitor_geometry)),
            Some(SourcePhase::WindowPick) => window,
            Some(SourcePhase::Menu) | None => return false,
        };
        let (source, geometry) = choice
            .map(|(source, geometry)| (Some(source), Some(geometry)))
            .unwrap_or((None, None));
        let changed = self.source != source || self.source_geometry != geometry;
        self.source = source;
        self.source_geometry = geometry;
        changed
    }

    pub fn selected_source(&self) -> Option<&halley_ipc::CaptureSource> {
        self.source.as_ref()
    }

    pub fn select_source(
        &mut self,
        source: halley_ipc::CaptureSource,
        geometry: Rectangle<i32, Logical>,
    ) -> bool {
        let valid = matches!(
            (self.phase, &source),
            (
                Some(SourcePhase::MonitorPick),
                halley_ipc::CaptureSource::Monitor { .. }
            ) | (
                Some(SourcePhase::WindowPick),
                halley_ipc::CaptureSource::Window { .. }
            )
        );
        if !valid {
            return false;
        }
        let changed =
            self.source.as_ref() != Some(&source) || self.source_geometry != Some(geometry);
        self.source = Some(source);
        self.source_geometry = Some(geometry);
        changed
    }

    pub fn take_selected(&mut self) -> Option<halley_ipc::CaptureSource> {
        let source = self.source.take()?;
        self.cancel();
        Some(source)
    }

    fn is_enabled(&self, index: usize) -> bool {
        SourceMode::ALL.get(index).is_some_and(|mode| match mode {
            SourceMode::Monitor => self.monitor_available(),
            SourceMode::Window => self.window_available(),
        })
    }
}

fn monitor_source(
    output_name: &str,
    geometry: Rectangle<i32, Logical>,
) -> halley_ipc::CaptureSource {
    halley_ipc::CaptureSource::Monitor {
        name: output_name.to_string(),
        x: geometry.loc.x,
        y: geometry.loc.y,
        width: geometry.size.w,
        height: geometry.size.h,
    }
}

pub fn layout(output: Rectangle<i32, Logical>) -> SourceMenuLayout {
    let width = BAR_WIDTH.min(output.size.w.max(1));
    let height = BAR_HEIGHT.min(output.size.h.max(1));
    let x = output.loc.x + (output.size.w - width) / 2;
    let y = output.loc.y + (output.size.h - height - BAR_BOTTOM_MARGIN).max(0);
    let bar = Rectangle::new((x, y).into(), (width, height).into());
    let slot_width = width / 2;
    let item = |index: i32| {
        let left = x + index * slot_width;
        let right = if index == 1 {
            x + width
        } else {
            left + slot_width
        };
        let horizontal_padding = ITEM_PADDING.min((right - left).max(0) / 2);
        let vertical_padding = ITEM_PADDING.min(height.max(0) / 2);
        Rectangle::new(
            (left + horizontal_padding, y + vertical_padding).into(),
            (
                right - left - horizontal_padding * 2,
                height - vertical_padding * 2,
            )
                .into(),
        )
    };
    SourceMenuLayout {
        bar,
        items: [item(0), item(1)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> Rectangle<i32, Logical> {
        Rectangle::new((1920, 0).into(), (1920, 1080).into())
    }

    #[test]
    fn chooser_uses_menu_then_explicit_pick_phase() {
        let mut chooser = SourceChooser::default();
        chooser.begin(
            halley_ipc::SOURCE_MONITOR | halley_ipc::SOURCE_WINDOW,
            "DP-2".to_string(),
            output(),
        );
        assert_eq!(chooser.phase(), Some(SourcePhase::Menu));
        assert!(chooser.activate(SourceMode::Window));
        assert_eq!(chooser.phase(), Some(SourcePhase::WindowPick));
        assert!(chooser.return_to_menu());
        assert!(chooser.activate(SourceMode::Monitor));
        assert_eq!(chooser.phase(), Some(SourcePhase::MonitorPick));
    }

    #[test]
    fn keyboard_selection_cannot_switch_source_type_or_confirm_without_a_choice() {
        let mut chooser = SourceChooser::default();
        chooser.begin(halley_ipc::SOURCE_WINDOW, "DP-2".into(), output());
        let monitor = monitor_source("DP-2", output());
        assert!(!chooser.select_source(monitor, output()));
        assert!(chooser.selected_source().is_none());
        assert!(chooser.take_selected().is_none());
        let window = halley_ipc::CaptureSource::Window {
            window_id: 7,
            width: 100,
            height: 100,
        };
        assert!(chooser.select_source(window.clone(), output()));
        assert_eq!(chooser.take_selected(), Some(window));
    }

    #[test]
    fn menu_keeps_unsupported_source_type_disabled() {
        let mut chooser = SourceChooser::default();
        chooser.begin(halley_ipc::SOURCE_WINDOW, "DP-2".to_string(), output());
        assert_eq!(chooser.selected(), 1);
        assert_eq!(chooser.phase(), Some(SourcePhase::WindowPick));
        assert!(!chooser.activate(SourceMode::Monitor));
        assert!(!chooser.activate(SourceMode::Window));
        assert!(!chooser.return_to_menu());
    }

    #[test]
    fn monitor_only_request_skips_the_category_menu() {
        let mut chooser = SourceChooser::default();
        chooser.begin(halley_ipc::SOURCE_MONITOR, "DP-2".to_string(), output());
        assert_eq!(chooser.phase(), Some(SourcePhase::MonitorPick));
        assert!(!chooser.return_to_menu());
        assert!(matches!(
            chooser.take_selected(),
            Some(halley_ipc::CaptureSource::Monitor { name, .. }) if name == "DP-2"
        ));
    }

    #[test]
    fn menu_layout_is_centered_near_output_bottom() {
        let layout = layout(output());
        assert_eq!(
            layout.bar,
            Rectangle::new((2700, 972).into(), (360, 80).into())
        );
        assert!(
            layout
                .items
                .iter()
                .all(|item| layout.bar.contains_rect(*item))
        );
    }
}
