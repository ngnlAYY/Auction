//! Worker 配置校验的模块装配；边界校验在子目录按 owner 持有，此处只重新导出 `pub(super)` 函数。

pub(super) mod auction;
pub(super) mod control;
pub(super) mod delivery;
pub(super) mod forum;
pub(super) mod fulfillment;
pub(super) mod mooglemail;
pub(super) mod operations;
pub(super) mod recovery;

pub(crate) use mooglemail::sender_allowlist_has_case_duplicates;
