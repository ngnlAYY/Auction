//! 同账号重登的偏好重读，同一表单只消费最近待处理的修订。

use dioxus::prelude::*;

/// 外壳权威确认同账号重登后推进；Cookie 持久期同步不会推进此修订。
#[derive(Clone, Copy)]
pub(in crate::surface) struct PreferenceReloadRevision(pub Signal<u64>);

/// 后台重读保留草稿；首次、显式刷新和确认写入重新建立基线。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::surface) enum PreferenceRead {
    Refresh,
    Background,
    Confirmed,
}

#[derive(Clone, Copy)]
pub(in crate::surface) struct PreferenceReload {
    requested: Option<PreferenceReloadRevision>,
    started: Signal<Option<u64>>,
}

impl PreferenceReload {
    /// 在稳定的账号表单中记录第一次和后续的偏好读取。
    pub(in crate::surface) fn use_tracking() -> Self {
        Self {
            requested: try_use_context::<PreferenceReloadRevision>(),
            started: use_signal(|| None),
        }
    }

    /// 首次读取或积压的重登修订。读取前须另行检查表单的会话和命令状态。
    pub(in crate::surface) fn pending_read(self) -> Option<PreferenceRead> {
        let revision = self.requested.map_or(0, |requested| (requested.0)());
        match *self.started.peek() {
            None => Some(PreferenceRead::Refresh),
            Some(started) if started != revision => Some(PreferenceRead::Background),
            Some(_) => None,
        }
    }

    /// 手动重读或确认写入后的读取也可消费积压的修订。
    pub(in crate::surface) fn mark_started(mut self) {
        let revision = self.requested.map_or(0, |requested| *requested.0.peek());
        self.started.set(Some(revision));
    }
}
