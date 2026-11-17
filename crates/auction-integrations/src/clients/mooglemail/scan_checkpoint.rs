//! 增量扫描检查点的推进规则；Sent/Read Mail 维护（sender）与 gateway 进程内检查点共用，
//! 放在两者之外，避免 gateway 依赖 sender。

use auction_application::mail::{
    MailScanCheckpoint, MailScanWatermark, advance_mail_scan_watermark,
};
use auction_time::Timestamp;

/// 水位只前进，本轮没有新水位时沿用旧值；只有完整扫描（`full_at` 有值）才推进
/// `last_full_scan_at`。
pub(crate) fn advance_scan_checkpoint(
    previous: Option<&MailScanCheckpoint>,
    newest: Option<MailScanWatermark>,
    full_at: Option<Timestamp>,
) -> MailScanCheckpoint {
    let previous_watermark = previous.and_then(|checkpoint| checkpoint.watermark.as_ref());
    let watermark = match newest {
        Some(newest) => Some(advance_mail_scan_watermark(previous_watermark, &newest)),
        None => previous_watermark.cloned(),
    };
    let last_full_scan_at =
        full_at.or_else(|| previous.and_then(|checkpoint| checkpoint.last_full_scan_at));
    MailScanCheckpoint { watermark, last_full_scan_at }
}
