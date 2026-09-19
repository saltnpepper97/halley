use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const TRANSITION_DURATION: Duration = Duration::from_millis(180);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    Success,
    Error,
}

#[derive(Clone, Debug)]
struct Notification {
    output: String,
    message: String,
    kind: NotificationKind,
    shown_at: Duration,
    enter_from: f32,
    expires_at: Duration,
    dismissed: Option<(Duration, f32)>,
    expiry_notified: bool,
}

impl Notification {
    fn mix(&self, now: Duration) -> f32 {
        if let Some((dismissed_at, from)) = self.dismissed {
            return from * (1.0 - transition_progress(now.saturating_sub(dismissed_at)));
        }
        if now >= self.expires_at {
            return 1.0 - transition_progress(now.saturating_sub(self.expires_at));
        }
        self.enter_from
            + (1.0 - self.enter_from) * transition_progress(now.saturating_sub(self.shown_at))
    }

    fn finished(&self, now: Duration) -> bool {
        let end = self.dismissed.map(|(at, _)| at).unwrap_or(self.expires_at) + TRANSITION_DURATION;
        now >= end
    }

    fn animating(&self, now: Duration) -> bool {
        if self.finished(now) {
            return false;
        }
        now < self.shown_at + TRANSITION_DURATION
            || self.dismissed.is_some()
            || now >= self.expires_at
    }
}

#[derive(Clone, Debug)]
struct ZoomIndicator {
    scale: f32,
    shown_at: Duration,
    enter_from: f32,
    expires_at: Duration,
    fade_duration: Duration,
    expiry_notified: bool,
}

impl ZoomIndicator {
    fn mix(&self, now: Duration) -> f32 {
        if now >= self.expires_at {
            return 1.0
                - transition_progress_for(now.saturating_sub(self.expires_at), self.fade_duration);
        }
        self.enter_from
            + (1.0 - self.enter_from)
                * transition_progress_for(now.saturating_sub(self.shown_at), self.fade_duration)
    }

    fn finished(&self, now: Duration) -> bool {
        now >= self.expires_at + self.fade_duration
    }

    fn animating(&self, now: Duration) -> bool {
        !self.finished(now)
            && (self.enter_from < 0.999 && now < self.shown_at + self.fade_duration
                || now >= self.expires_at)
    }
}

#[derive(Clone, Debug)]
struct ClusterIndicator {
    label: String,
    shown_at: Duration,
    expires_at: Duration,
    expiry_notified: bool,
}

impl ClusterIndicator {
    fn mix(&self, now: Duration) -> f32 {
        if now >= self.expires_at {
            return 1.0 - transition_progress(now.saturating_sub(self.expires_at));
        }
        transition_progress(now.saturating_sub(self.shown_at))
    }

    fn finished(&self, now: Duration) -> bool {
        now >= self.expires_at + TRANSITION_DURATION
    }

    fn animating(&self, now: Duration) -> bool {
        !self.finished(now) && (now < self.shown_at + TRANSITION_DURATION || now >= self.expires_at)
    }
}

#[derive(Clone, Debug)]
struct BasicsCard {
    output: String,
    modifier: String,
    shown_at: Duration,
    dismissed: Option<(Duration, f32)>,
}

impl BasicsCard {
    fn mix(&self, now: Duration) -> f32 {
        if let Some((dismissed_at, from)) = self.dismissed {
            return from * (1.0 - transition_progress(now.saturating_sub(dismissed_at)));
        }
        transition_progress(now.saturating_sub(self.shown_at))
    }

    fn finished(&self, now: Duration) -> bool {
        self.dismissed
            .is_some_and(|(at, _)| now >= at + TRANSITION_DURATION)
    }
}

#[derive(Clone, Debug)]
struct ClusterDeleteConfirmation {
    cluster_id: halley_core::cluster::ClusterId,
    output: String,
    name: String,
}

#[derive(Clone, Debug, Default)]
pub struct OverlayManager {
    exit: bool,
    cluster_delete: Option<ClusterDeleteConfirmation>,
    basics: Option<BasicsCard>,
    notification: Option<Notification>,
    zoom_indicators: HashMap<String, ZoomIndicator>,
    cluster_indicators: HashMap<String, ClusterIndicator>,
}

#[derive(Clone, Debug)]
pub struct NotificationSnapshot {
    pub message: String,
    pub kind: NotificationKind,
    pub mix: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ZoomIndicatorSnapshot {
    pub scale: f32,
    pub mix: f32,
}

#[derive(Clone, Debug)]
pub struct ClusterIndicatorSnapshot {
    pub label: String,
    pub mix: f32,
}

#[derive(Clone, Debug)]
pub struct ConfirmationSnapshot {
    pub title: String,
    pub message: String,
    pub confirm_label: &'static str,
}

#[derive(Clone, Debug)]
pub struct BasicsCardSnapshot {
    /// The configured base modifier, already remapped for this backend, so the
    /// card names the chords the session actually listens for.
    pub modifier: String,
    pub mix: f32,
}

#[derive(Clone, Debug, Default)]
pub struct OverlaySnapshot {
    pub exit_mix: Option<f32>,
    pub confirmation: Option<ConfirmationSnapshot>,
    pub basics: Option<BasicsCardSnapshot>,
    pub notification: Option<NotificationSnapshot>,
    pub zoom_indicator: Option<ZoomIndicatorSnapshot>,
    pub cluster_indicator: Option<ClusterIndicatorSnapshot>,
}

impl OverlayManager {
    pub fn confirmation_modal_active(&self) -> bool {
        self.exit || self.cluster_delete.is_some()
    }

    pub fn exit_modal_active(&self) -> bool {
        self.exit
    }

    pub fn cluster_delete_modal_active(&self) -> bool {
        self.cluster_delete.is_some()
    }

    pub fn show_exit(&mut self, _now: Duration) -> bool {
        if self.confirmation_modal_active() {
            return false;
        }
        self.exit = true;
        true
    }

    pub fn cancel_exit(&mut self, _now: Duration) -> bool {
        std::mem::take(&mut self.exit)
    }

    pub fn show_cluster_delete(
        &mut self,
        cluster_id: halley_core::cluster::ClusterId,
        output: String,
        name: String,
    ) -> bool {
        if self.confirmation_modal_active() {
            return false;
        }
        self.cluster_delete = Some(ClusterDeleteConfirmation {
            cluster_id,
            output,
            name,
        });
        true
    }

    pub fn cancel_cluster_delete(&mut self) -> bool {
        self.cluster_delete.take().is_some()
    }

    pub fn take_cluster_delete(&mut self) -> Option<(halley_core::cluster::ClusterId, String)> {
        self.cluster_delete
            .take()
            .map(|confirmation| (confirmation.cluster_id, confirmation.output))
    }

    /// Shows the one-time basics card on `output`. Returns whether it was newly
    /// shown; re-showing while it is already visible is a no-op.
    pub fn show_basics_card(&mut self, output: String, modifier: String, now: Duration) -> bool {
        if self.basics_card_visible() {
            return false;
        }
        self.basics = Some(BasicsCard {
            output,
            modifier,
            shown_at: now,
            dismissed: None,
        });
        true
    }

    /// Whether a basics card is on screen, including while it fades out.
    pub fn basics_card_visible(&self) -> bool {
        self.basics.is_some()
    }

    /// Whether the card should still swallow its dismissal keys and the first
    /// pointer press. Once dismissed it stops intercepting input immediately,
    /// even though it keeps fading out for one transition.
    pub fn basics_card_accepts_input(&self) -> bool {
        self.basics
            .as_ref()
            .is_some_and(|card| card.dismissed.is_none())
    }

    pub fn dismiss_basics_card(&mut self, now: Duration) -> bool {
        let Some(card) = self.basics.as_mut() else {
            return false;
        };
        if card.dismissed.is_some() {
            return false;
        }
        card.dismissed = Some((now, card.mix(now)));
        true
    }

    pub fn show_config_success(
        &mut self,
        output: String,
        path: &Path,
        duration_ms: u64,
        now: Duration,
    ) {
        self.show_notification(
            output,
            format!("Configuration successfully loaded from {}", path.display()),
            NotificationKind::Success,
            duration_ms,
            now,
        );
    }

    pub fn show_config_error(&mut self, output: String, duration_ms: u64, now: Duration) {
        self.show_notification(
            output,
            "Current configuration was unable to load properly. Run `halleyctl config verify` to see why."
                .to_string(),
            NotificationKind::Error,
            duration_ms,
            now,
        );
    }

    pub fn show_screenshot_saved(
        &mut self,
        output: String,
        directory: &Path,
        duration_ms: u64,
        now: Duration,
    ) {
        self.show_notification(
            output,
            format!("Screenshot saved to {}", directory.display()),
            NotificationKind::Success,
            duration_ms,
            now,
        );
    }

    pub fn show_error(
        &mut self,
        output: String,
        message: impl Into<String>,
        duration_ms: u64,
        now: Duration,
    ) {
        self.show_notification(
            output,
            message.into(),
            NotificationKind::Error,
            duration_ms,
            now,
        );
    }

    pub fn clear_config_error(&mut self, now: Duration) -> bool {
        let Some(notification) = self.notification.as_mut() else {
            return false;
        };
        if notification.kind != NotificationKind::Error || notification.dismissed.is_some() {
            return false;
        }
        notification.dismissed = Some((now, notification.mix(now)));
        true
    }

    fn show_notification(
        &mut self,
        output: String,
        message: String,
        kind: NotificationKind,
        duration_ms: u64,
        now: Duration,
    ) {
        let enter_from = self
            .notification
            .as_ref()
            .filter(|notification| !notification.finished(now))
            .map(|notification| notification.mix(now))
            .unwrap_or(0.0);
        self.notification = Some(Notification {
            output,
            message,
            kind,
            shown_at: now,
            enter_from,
            expires_at: now + Duration::from_millis(duration_ms.max(1)),
            dismissed: None,
            expiry_notified: false,
        });
    }

    pub fn show_zoom_indicator(
        &mut self,
        output: &str,
        scale: f32,
        config: &halley_config::ZoomIndicator,
        now: Duration,
    ) -> bool {
        if !config.enabled {
            return self.zoom_indicators.remove(output).is_some();
        }

        let (shown_at, enter_from) = self
            .zoom_indicators
            .get(output)
            .filter(|indicator| !indicator.finished(now))
            .map(|indicator| {
                if now >= indicator.expires_at {
                    (now, indicator.mix(now))
                } else {
                    (indicator.shown_at, indicator.enter_from)
                }
            })
            .unwrap_or((now, 1.0));
        self.zoom_indicators.insert(
            output.to_string(),
            ZoomIndicator {
                scale,
                shown_at,
                enter_from,
                expires_at: now + Duration::from_millis(config.hold_duration_ms),
                fade_duration: Duration::from_millis(config.fade_duration_ms),
                expiry_notified: false,
            },
        );
        true
    }

    pub fn show_cluster_indicator(
        &mut self,
        output: &str,
        name: &str,
        layout: halley_core::cluster::layout::ClusterWorkspaceLayoutKind,
        now: Duration,
    ) {
        let layout = match layout {
            halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling => "Tiling",
            halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Stacking => "Stacking",
        };
        self.cluster_indicators.insert(
            output.to_string(),
            ClusterIndicator {
                label: format!("{}  ·  {layout}", name.trim()),
                shown_at: now,
                expires_at: now + Duration::from_millis(900),
                expiry_notified: false,
            },
        );
    }

    pub fn reload_zoom_indicator(&mut self, config: &halley_config::ZoomIndicator) -> bool {
        if config.enabled || self.zoom_indicators.is_empty() {
            return false;
        }
        self.zoom_indicators.clear();
        true
    }

    pub fn remove_output(&mut self, output: &str) {
        if self
            .basics
            .as_ref()
            .is_some_and(|card| card.output == output)
        {
            self.basics = None;
        }
        self.zoom_indicators.remove(output);
        self.cluster_indicators.remove(output);
    }

    pub fn snapshot(&self, output: &str, now: Duration) -> OverlaySnapshot {
        OverlaySnapshot {
            exit_mix: self.exit.then_some(1.0),
            confirmation: self.cluster_delete.as_ref().and_then(|confirmation| {
                (confirmation.output == output).then(|| ConfirmationSnapshot {
                    title: format!("Delete {}?", confirmation.name.trim()),
                    message: "Its windows will return to the Field. Applications will remain open."
                        .to_string(),
                    confirm_label: "delete",
                })
            }),
            basics: self.basics.as_ref().and_then(|card| {
                (card.output == output && !card.finished(now)).then(|| BasicsCardSnapshot {
                    modifier: card.modifier.clone(),
                    mix: card.mix(now),
                })
            }),
            notification: self.notification.as_ref().and_then(|notification| {
                (notification.output == output && !notification.finished(now)).then(|| {
                    NotificationSnapshot {
                        message: notification.message.clone(),
                        kind: notification.kind,
                        mix: notification.mix(now),
                    }
                })
            }),
            zoom_indicator: self.zoom_indicators.get(output).and_then(|indicator| {
                (!indicator.finished(now)).then(|| ZoomIndicatorSnapshot {
                    scale: indicator.scale,
                    mix: indicator.mix(now),
                })
            }),
            cluster_indicator: self.cluster_indicators.get(output).and_then(|indicator| {
                (!indicator.finished(now)).then(|| ClusterIndicatorSnapshot {
                    label: indicator.label.clone(),
                    mix: indicator.mix(now),
                })
            }),
        }
    }

    pub fn animating(&self, now: Duration) -> bool {
        self.basics
            .as_ref()
            .is_some_and(|card| !card.finished(now) && card.mix(now) < 1.0)
            || self
                .notification
                .as_ref()
                .is_some_and(|notification| notification.animating(now))
            || self
                .zoom_indicators
                .values()
                .any(|indicator| indicator.animating(now))
            || self
                .cluster_indicators
                .values()
                .any(|indicator| indicator.animating(now))
    }

    /// Timer-side lifecycle update. Returns true when a frame must be queued
    /// to begin an expiry fade or erase a completed overlay.
    pub fn wakeup(&mut self, now: Duration) -> bool {
        let mut redraw = false;
        if self.basics.as_ref().is_some_and(|card| card.finished(now)) {
            self.basics = None;
            redraw = true;
        }
        if let Some(notification) = self.notification.as_mut()
            && notification.dismissed.is_none()
            && now >= notification.expires_at
            && !notification.expiry_notified
        {
            notification.expiry_notified = true;
            redraw = true;
        }
        if self
            .notification
            .as_ref()
            .is_some_and(|notification| notification.finished(now))
        {
            self.notification = None;
            redraw = true;
        }
        for indicator in self.zoom_indicators.values_mut() {
            if now >= indicator.expires_at && !indicator.expiry_notified {
                indicator.expiry_notified = true;
                redraw = true;
            }
        }
        let before = self.zoom_indicators.len();
        self.zoom_indicators
            .retain(|_, indicator| !indicator.finished(now));
        redraw |= self.zoom_indicators.len() != before;
        for indicator in self.cluster_indicators.values_mut() {
            if now >= indicator.expires_at && !indicator.expiry_notified {
                indicator.expiry_notified = true;
                redraw = true;
            }
        }
        let before = self.cluster_indicators.len();
        self.cluster_indicators
            .retain(|_, indicator| !indicator.finished(now));
        redraw |= self.cluster_indicators.len() != before;
        redraw
    }
}

fn transition_progress(elapsed: Duration) -> f32 {
    transition_progress_for(elapsed, TRANSITION_DURATION)
}

fn transition_progress_for(elapsed: Duration, duration: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_error_replaces_message_lifetime_without_flashing_out() {
        let mut overlays = OverlayManager::default();
        overlays.show_config_error("DP-1".into(), 9_000, Duration::ZERO);
        let halfway = Duration::from_millis(100);
        let before = overlays.snapshot("DP-1", halfway).notification.unwrap().mix;

        overlays.show_config_error("DP-1".into(), 9_000, halfway);
        let after = overlays.snapshot("DP-1", halfway).notification.unwrap().mix;

        assert_eq!(before, after);
        assert!(after > 0.0);
    }

    #[test]
    fn valid_reload_only_dismisses_an_error() {
        let mut overlays = OverlayManager::default();
        overlays.show_config_success(
            "DP-1".into(),
            Path::new("/tmp/halley.rune"),
            4_000,
            Duration::ZERO,
        );
        assert!(!overlays.clear_config_error(Duration::from_millis(10)));

        overlays.show_config_error("DP-1".into(), 9_000, Duration::from_millis(20));
        assert!(overlays.clear_config_error(Duration::from_millis(30)));
    }

    #[test]
    fn screenshot_success_names_the_destination_directory() {
        let mut overlays = OverlayManager::default();
        overlays.show_screenshot_saved(
            "DP-1".into(),
            Path::new("/home/test/Pictures/Screenshots"),
            4_000,
            Duration::ZERO,
        );

        let notification = overlays
            .snapshot("DP-1", Duration::from_millis(180))
            .notification
            .unwrap();
        assert_eq!(
            notification.message,
            "Screenshot saved to /home/test/Pictures/Screenshots"
        );
        assert_eq!(notification.kind, NotificationKind::Success);
    }

    #[test]
    fn exit_confirmation_is_instant_and_idempotent() {
        let mut overlays = OverlayManager::default();
        assert!(overlays.show_exit(Duration::ZERO));
        assert_eq!(
            overlays.snapshot("DP-1", Duration::ZERO).exit_mix,
            Some(1.0)
        );
        assert!(!overlays.animating(Duration::ZERO));
        assert!(!overlays.show_exit(Duration::from_millis(1)));
        assert!(overlays.exit_modal_active());

        assert!(overlays.cancel_exit(Duration::from_millis(20)));
        assert_eq!(
            overlays
                .snapshot("DP-1", Duration::from_millis(20))
                .exit_mix,
            None
        );
        assert!(!overlays.exit_modal_active());
        assert!(!overlays.cancel_exit(Duration::from_millis(21)));
    }

    #[test]
    fn populated_cluster_delete_explains_that_windows_survive() {
        let mut overlays = OverlayManager::default();
        let cluster = halley_core::cluster::ClusterId::new(7);
        assert!(overlays.show_cluster_delete(cluster, "DP-1".into(), "Work".into()));
        assert!(overlays.confirmation_modal_active());
        assert!(!overlays.show_exit(Duration::ZERO));
        assert!(
            overlays
                .snapshot("DP-2", Duration::ZERO)
                .confirmation
                .is_none()
        );

        let confirmation = overlays
            .snapshot("DP-1", Duration::ZERO)
            .confirmation
            .expect("confirmation");
        assert_eq!(confirmation.title, "Delete Work?");
        assert!(confirmation.message.contains("return to the Field"));
        assert!(confirmation.message.contains("remain open"));
        assert_eq!(
            overlays.take_cluster_delete(),
            Some((cluster, "DP-1".into()))
        );
        assert!(!overlays.confirmation_modal_active());
    }

    #[test]
    fn cluster_indicator_names_the_workspace_and_layout_then_expires() {
        let mut overlays = OverlayManager::default();
        overlays.show_cluster_indicator(
            "DP-1",
            "Work",
            halley_core::cluster::layout::ClusterWorkspaceLayoutKind::Tiling,
            Duration::ZERO,
        );

        let visible = overlays
            .snapshot("DP-1", Duration::from_millis(180))
            .cluster_indicator
            .unwrap();
        assert_eq!(visible.label, "Work  ·  Tiling");
        assert_eq!(visible.mix, 1.0);
        assert!(
            overlays
                .snapshot("DP-2", Duration::from_millis(180))
                .cluster_indicator
                .is_none()
        );
        assert!(overlays.wakeup(Duration::from_millis(900)));
        assert!(
            overlays
                .snapshot("DP-1", Duration::from_millis(1_080))
                .cluster_indicator
                .is_none()
        );
    }

    #[test]
    fn zoom_indicator_holds_after_activity_then_fades() {
        let mut overlays = OverlayManager::default();
        let config = halley_config::ZoomIndicator::default();
        overlays.show_zoom_indicator("DP-1", 0.75, &config, Duration::ZERO);

        let initial = overlays
            .snapshot("DP-1", Duration::ZERO)
            .zoom_indicator
            .unwrap();
        assert_eq!(initial.scale, 0.75);
        assert_eq!(initial.mix, 1.0);
        assert_eq!(
            overlays
                .snapshot("DP-1", Duration::from_millis(749))
                .zoom_indicator
                .unwrap()
                .mix,
            1.0
        );
        assert!(
            overlays
                .snapshot("DP-1", Duration::from_millis(840))
                .zoom_indicator
                .unwrap()
                .mix
                < 1.0
        );
        assert!(
            overlays
                .snapshot("DP-1", Duration::from_millis(930))
                .zoom_indicator
                .is_none()
        );

        let mut overlays = OverlayManager::default();
        overlays.show_zoom_indicator("DP-1", 0.75, &config, Duration::ZERO);
        assert!(!overlays.animating(Duration::from_millis(749)));
        assert!(overlays.wakeup(Duration::from_millis(750)));
        assert!(overlays.animating(Duration::from_millis(750)));
        assert!(overlays.wakeup(Duration::from_millis(930)));
    }

    #[test]
    fn repeated_zoom_activity_extends_the_hold_and_updates_the_live_scale() {
        let mut overlays = OverlayManager::default();
        let config = halley_config::ZoomIndicator::default();
        overlays.show_zoom_indicator("DP-1", 0.90, &config, Duration::ZERO);
        overlays.show_zoom_indicator("DP-1", 0.70, &config, Duration::from_millis(700));

        let snapshot = overlays
            .snapshot("DP-1", Duration::from_millis(1_000))
            .zoom_indicator
            .unwrap();
        assert_eq!(snapshot.scale, 0.70);
        assert_eq!(snapshot.mix, 1.0);
    }

    #[test]
    fn activity_during_fade_reverses_without_flashing_or_affecting_other_outputs() {
        let mut overlays = OverlayManager::default();
        let config = halley_config::ZoomIndicator::default();
        overlays.show_zoom_indicator("DP-1", 0.80, &config, Duration::ZERO);
        overlays.show_zoom_indicator("DP-2", 0.60, &config, Duration::ZERO);
        let reactivated_at = Duration::from_millis(840);
        let before = overlays
            .snapshot("DP-1", reactivated_at)
            .zoom_indicator
            .unwrap()
            .mix;

        overlays.show_zoom_indicator("DP-1", 0.75, &config, reactivated_at);
        let after = overlays
            .snapshot("DP-1", reactivated_at)
            .zoom_indicator
            .unwrap();

        assert_eq!(after.mix, before);
        assert_eq!(after.scale, 0.75);
        assert_eq!(
            overlays
                .snapshot("DP-2", reactivated_at)
                .zoom_indicator
                .unwrap()
                .scale,
            0.60
        );
        assert!(
            overlays
                .snapshot("DP-1", Duration::from_millis(900))
                .zoom_indicator
                .unwrap()
                .mix
                > before
        );
    }

    #[test]
    fn disabling_or_removing_an_output_clears_zoom_indicator_state() {
        let mut overlays = OverlayManager::default();
        let mut config = halley_config::ZoomIndicator::default();
        overlays.show_zoom_indicator("DP-1", 0.75, &config, Duration::ZERO);
        overlays.remove_output("DP-1");
        assert!(
            overlays
                .snapshot("DP-1", Duration::ZERO)
                .zoom_indicator
                .is_none()
        );

        overlays.show_zoom_indicator("DP-1", 0.75, &config, Duration::ZERO);
        config.enabled = false;
        assert!(overlays.reload_zoom_indicator(&config));
        assert!(
            overlays
                .snapshot("DP-1", Duration::ZERO)
                .zoom_indicator
                .is_none()
        );

        assert!(!overlays.show_zoom_indicator("DP-1", 0.75, &config, Duration::ZERO));
        assert!(
            overlays
                .snapshot("DP-1", Duration::ZERO)
                .zoom_indicator
                .is_none()
        );
    }

    #[test]
    fn basics_card_is_visible_only_on_its_output_and_fades_when_dismissed() {
        let mut overlays = OverlayManager::default();
        assert!(overlays.show_basics_card("DP-1".into(), "Super".into(), Duration::ZERO));
        assert!(overlays.basics_card_visible());
        assert!(overlays.basics_card_accepts_input());
        assert!(
            !overlays.show_basics_card("DP-2".into(), "Super".into(), Duration::ZERO),
            "showing an already visible card is a no-op"
        );

        let card = overlays.snapshot("DP-1", Duration::ZERO).basics.unwrap();
        assert_eq!(card.modifier, "Super");
        assert_eq!(
            card.mix, 0.0,
            "the card fades in instead of appearing abruptly"
        );
        assert!(overlays.animating(Duration::ZERO));
        assert_eq!(
            overlays
                .snapshot("DP-1", Duration::from_millis(180))
                .basics
                .unwrap()
                .mix,
            1.0
        );
        assert!(
            overlays.snapshot("DP-2", Duration::ZERO).basics.is_none(),
            "the card only renders on the output that owns it"
        );

        assert!(overlays.dismiss_basics_card(Duration::from_millis(500)));
        assert!(
            !overlays.basics_card_accepts_input(),
            "a dismissed card stops intercepting input immediately"
        );
        let fading = overlays
            .snapshot("DP-1", Duration::from_millis(590))
            .basics
            .unwrap();
        assert!(fading.mix < 1.0);
        assert!(
            overlays
                .snapshot("DP-1", Duration::from_millis(680))
                .basics
                .is_none()
        );
        assert!(overlays.wakeup(Duration::from_millis(680)));
        assert!(!overlays.basics_card_visible());
        assert!(!overlays.dismiss_basics_card(Duration::from_millis(700)));
    }

    /// Opening the card again stays independent of the one-time dismissal
    /// state: the overlay itself remembers nothing about first-run eligibility.
    #[test]
    fn basics_card_can_be_reopened_after_being_dismissed() {
        let mut overlays = OverlayManager::default();
        overlays.show_basics_card("DP-1".into(), "Alt".into(), Duration::ZERO);
        overlays.dismiss_basics_card(Duration::from_millis(10));
        assert!(overlays.wakeup(Duration::from_millis(200)));

        assert!(overlays.show_basics_card("DP-1".into(), "Alt".into(), Duration::from_millis(300)));
        assert!(overlays.basics_card_accepts_input());
        assert_eq!(
            overlays
                .snapshot("DP-1", Duration::from_millis(480))
                .basics
                .unwrap()
                .modifier,
            "Alt"
        );
    }

    #[test]
    fn removing_the_owning_output_drops_the_basics_card() {
        let mut overlays = OverlayManager::default();
        overlays.show_basics_card("DP-1".into(), "Super".into(), Duration::ZERO);
        overlays.remove_output("DP-2");
        assert!(overlays.basics_card_visible());

        overlays.remove_output("DP-1");
        assert!(!overlays.basics_card_visible());
        assert!(!overlays.basics_card_accepts_input());
    }

    #[test]
    fn a_confirmation_modal_does_not_hide_the_basics_card_by_itself() {
        let mut overlays = OverlayManager::default();
        overlays.show_basics_card("DP-1".into(), "Super".into(), Duration::ZERO);
        assert!(overlays.show_exit(Duration::ZERO));
        // The card is a non-blocking overlay: it keeps its slot while a
        // confirmation modal owns the input, so the two never interleave.
        assert!(overlays.snapshot("DP-1", Duration::ZERO).basics.is_some());
        assert!(overlays.snapshot("DP-1", Duration::ZERO).exit_mix.is_some());
    }
}
