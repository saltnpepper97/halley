use halley_core::cluster::ClusterId;

#[derive(Clone, Debug)]
pub(crate) struct OverlayBannerState {
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) visible: bool,
    pub(crate) mix: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct OverlayBannerSnapshot {
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) mix: f32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OverlayToastState {
    pub(crate) message: Option<String>,
    pub(crate) visible_until_ms: u64,
    pub(crate) mix: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct OverlayToastSnapshot {
    pub(crate) message: String,
    pub(crate) mix: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClusterBloomAnimState {
    pub(crate) cluster_id: Option<ClusterId>,
    pub(crate) visible: bool,
    pub(crate) mix: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ClusterBloomAnimSnapshot {
    pub(crate) cluster_id: ClusterId,
    pub(crate) mix: f32,
}
