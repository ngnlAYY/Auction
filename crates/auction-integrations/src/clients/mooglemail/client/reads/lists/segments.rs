//! 长列表扫描的分段游标：纯计算，只决定每段读取哪些页，不缓存列表内容。

use std::ops::Range;

/// 长列表扫描的分段游标；每段之间必须释放成员关系读锁，让详情 GET 有机会推进。
#[derive(Debug, Clone)]
pub(super) struct SegmentPlan {
    total_pages: usize,
    segment_pages: usize,
    next_page: usize,
}

impl SegmentPlan {
    /// `segment_pages` 必须为正；段数由 `total_pages` 决定。
    pub(super) fn new(total_pages: usize, segment_pages: usize) -> Self {
        assert!(segment_pages > 0, "segment_pages must be positive");
        Self { total_pages, segment_pages, next_page: 0 }
    }

    /// 返回下一个不重叠的页区间；耗尽后返回 `None`。
    pub(super) fn next_segment(&mut self) -> Option<Range<usize>> {
        if self.next_page >= self.total_pages {
            return None;
        }
        let start = self.next_page;
        let end = start.saturating_add(self.segment_pages).min(self.total_pages);
        self.next_page = end;
        Some(start..end)
    }
}

#[cfg(test)]
mod tests {
    use super::SegmentPlan;

    #[test]
    fn segment_plan_partitions_pages_without_overlap() {
        let mut plan = SegmentPlan::new(10, 4);
        assert_eq!(plan.next_segment(), Some(0..4));
        assert_eq!(plan.next_segment(), Some(4..8));
        assert_eq!(plan.next_segment(), Some(8..10));
        assert_eq!(plan.next_segment(), None);

        let mut empty = SegmentPlan::new(0, 4);
        assert_eq!(empty.next_segment(), None);

        let mut short = SegmentPlan::new(2, 8);
        assert_eq!(short.next_segment(), Some(0..2));
        assert_eq!(short.next_segment(), None);
    }
}
