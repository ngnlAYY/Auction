//! MoogleMail Sent / Inbox / Read Mail 的 MID 水位与增量前缀扫描策略。
//!
//! 水位表示“可作为下一轮前缀起点的最高 MID 与列表时间”；完整校验是否完成单独记录，
//! 它永远不是发送、收取、领取或已读证明。任何缺失、锚点消失、分页重复或顺序
//! 违反都必须回退完整扫描；完整扫描超限只记录告警，不能阻断发送或轮询。
//!
//! 本模块只含闭集分类与纯决策，不读取配置、不访问远端，也不保存正文或凭据。

use chrono::Duration;

/// 单个 MID 允许的最大字节数；与数据库与轮询游标边界一致。
pub const MAX_MAIL_SCAN_ID_BYTES: usize = 256;

/// 每日完整校验的默认间隔；部署可在 `[worker.mooglemail]
/// mail_scan_daily_full_interval_seconds` 覆盖，缺失时保持 24 小时。
pub const MAIL_SCAN_DAILY_FULL_INTERVAL: Duration = Duration::hours(24);

mod observe;
mod plan;
mod prefix;
#[cfg(test)]
mod tests;
pub use observe::{
    MailScanEvent, MailScanObserver, MailScanOutcome, MailScanShadow, compare_mail_scan_mids,
    noop_mail_scan_observer, numeric_mail_scan_id,
};
pub use plan::{
    MailScanAnchor, MailScanCheckpoint, MailScanMode, MailScanPlan, MailScanReason, MailScanStream,
    MailScanWatermark, plan_mail_scan,
};
pub use prefix::{MailPrefixOutcome, MailPrefixPage, MailPrefixScan, advance_mail_scan_watermark};
