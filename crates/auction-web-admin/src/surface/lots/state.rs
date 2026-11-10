//! Admin lot 页面使用的短生命周期交互状态。

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LotDialog {
    Reprice,
    Withdraw,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LotUpdateNotice {
    Repriced,
    Withdrawn,
}
