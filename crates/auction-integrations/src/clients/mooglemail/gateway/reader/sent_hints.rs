//! 已知 MID 的 Sent 列表页码提示：只决定从哪一页开始找，不参与任何证明。
//!
//! 履约义务等待收件人领取期间会反复按 MID 读取同一封 Sent 邮件；新邮件总是插在第 0 页，
//! 旧邮件只会逐渐后移，每次都从第 0 页翻起会把页面加载放大数倍。提示命中时直接从上次
//! 所在页读起；未命中时调用方仍完整重扫，“判缺失必须到尾页”的结论只来自完整扫描。
//! 提示只在进程内，丢失或过期最坏是多翻几页（不变量 9：缓存不决定证明或业务结果）。

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

use super::MoogleMailWorld;

/// 同时追踪的 MID 上限；超出时整体清空，等待中的义务下一轮重新建立提示。
const MAXIMUM_HINTS: usize = 1024;

#[derive(Default)]
pub(super) struct SentPageHints {
    pages: Mutex<HashMap<(MoogleMailWorld, String), usize>>,
}

impl SentPageHints {
    fn pages(&self) -> MutexGuard<'_, HashMap<(MoogleMailWorld, String), usize>> {
        // 提示只影响翻页起点；锁中毒时沿用残留内容，不能把读取升级成进程故障。
        self.pages.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 上次观察到该 MID 的页码；没有提示时返回 `None`，调用方从第 0 页开始。
    pub(super) fn start_page(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
    ) -> Option<usize> {
        self.pages().get(&(world, external_mail_id.to_owned())).copied()
    }

    pub(super) fn record(&self, world: MoogleMailWorld, external_mail_id: &str, page: usize) {
        let mut pages = self.pages();
        if pages.len() >= MAXIMUM_HINTS
            && !pages.contains_key(&(world, external_mail_id.to_owned()))
        {
            pages.clear();
        }
        pages.insert((world, external_mail_id.to_owned()), page);
    }

    /// 完整扫描确认缺失后丢弃提示，避免下次从不再相关的页码开始。
    pub(super) fn forget(&self, world: MoogleMailWorld, external_mail_id: &str) {
        self.pages().remove(&(world, external_mail_id.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::{MAXIMUM_HINTS, SentPageHints};
    use crate::clients::mooglemail::MoogleMailWorld;

    #[test]
    fn hints_are_scoped_by_world_and_mid() {
        let hints = SentPageHints::default();
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "100"), None);
        hints.record(MoogleMailWorld::Main, "100", 3);
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "100"), Some(3));
        assert_eq!(hints.start_page(MoogleMailWorld::Isekai, "100"), None);
        hints.record(MoogleMailWorld::Main, "100", 4);
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "100"), Some(4));
        hints.forget(MoogleMailWorld::Main, "100");
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "100"), None);
    }

    #[test]
    fn hint_table_is_bounded() {
        let hints = SentPageHints::default();
        for mid in 0..MAXIMUM_HINTS {
            hints.record(MoogleMailWorld::Main, &mid.to_string(), 1);
        }
        // 已有键原地更新，不触发清空。
        hints.record(MoogleMailWorld::Main, "0", 2);
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "1"), Some(1));
        // 新键超出上限时清空后重新记录。
        hints.record(MoogleMailWorld::Main, "new", 5);
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "1"), None);
        assert_eq!(hints.start_page(MoogleMailWorld::Main, "new"), Some(5));
    }
}
