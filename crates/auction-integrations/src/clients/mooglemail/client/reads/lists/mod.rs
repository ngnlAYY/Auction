//! 三种真实邮箱列表的有界完整扫描；列表身份与页数一起缓存，缺页不会伪装为完整。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use chrono::{DateTime, Utc};
use tokio::sync::OnceCell;
use tokio::sync::OwnedSemaphorePermit;

use super::read_unavailable;
use crate::clients::mooglemail::client::MoogleMailClient;
use crate::clients::mooglemail::parser::{MoogleMailSentSummary, MoogleMailSummary};
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

mod incoming;
mod sent;

/// 每次持有成员关系读锁的最大页数；段间必须释放，详情 GET 才有机会推进。
const INCOMING_SCAN_SEGMENT_PAGES: usize = 4;

type ListCell<T> = Arc<OnceCell<Arc<T>>>;

#[derive(Default)]
pub(super) struct ListCells {
    sent: StdMutex<HashMap<usize, ListCell<MoogleMailSentList>>>,
    incoming: StdMutex<HashMap<(bool, usize, u64), ListCell<MoogleMailIncomingList>>>,
}

pub(crate) struct MoogleMailSentList {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) pages_scanned: usize,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

/// 只枚举比基线边界更新的 Sent 前缀；`bounded` 表示更旧的邮件全部已在基线中。
pub(crate) struct MoogleMailSentPrefix {
    pub(crate) summaries: Vec<MoogleMailSentSummary>,
    pub(crate) bounded: bool,
    /// 当前列表中实际命中的基线 MID 时间；None 表示未找到基线行。
    pub(crate) anchor_sent_at: Option<DateTime<Utc>>,
    _bytes: OwnedSemaphorePermit,
}

pub(crate) struct MoogleMailIncomingList {
    pub(crate) summaries: Vec<MoogleMailSummary>,
    pub(crate) complete: bool,
    _bytes: OwnedSemaphorePermit,
}

/// 从持久水位继续的有界前缀；`anchor_found` 才表示“更新的邮件已全部枚举”。
/// 本结果只用于缩小观察窗口，永远不是收取、已读或发送证明。
pub(crate) struct MoogleMailIncomingPrefix {
    pub(crate) summaries: Vec<MoogleMailSummary>,
    pub(crate) anchor_found: bool,
    pub(crate) newest: Option<auction_application::mail::MailScanWatermark>,
    /// 回退原因；调用方据此记录指标并完整扫描。
    pub(crate) issue: Option<auction_application::mail::MailScanReason>,
    pub(crate) pages_scanned: usize,
    _bytes: OwnedSemaphorePermit,
}

impl MoogleMailClient {
    pub(super) fn retain_list_bytes(
        &self,
        bytes: usize,
        world: MoogleMailWorld,
    ) -> Result<OwnedSemaphorePermit, MoogleMailError> {
        let bytes = u32::try_from(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "convert_body_budget",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })?;
        Arc::clone(&self.reads.bytes).try_acquire_many_owned(bytes).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "reserve_body_budget",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            read_unavailable(world)
        })
    }
}

pub(super) fn validate_pages(
    maximum_pages: usize,
    world: MoogleMailWorld,
) -> Result<(), MoogleMailError> {
    if !(1..=100).contains(&maximum_pages) {
        return Err(MoogleMailError::new(
            MoogleMailOperation::Inbox,
            world,
            MoogleMailErrorCategory::Configuration,
            false,
        ));
    }
    Ok(())
}
