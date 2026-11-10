//! 恢复核对的列表枚举：按 cut-off 只读列表摘要，不读详情、不改变任何事实。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use chrono::{DateTime, Timelike, Utc};

use crate::diagnostics as safe_diagnostics;

use super::error::fetch_budget_exhausted;
use super::*;

impl MoogleMailGateway {
    /// 恢复核对：枚举 `cutoff_at` 之后的 Sent 摘要；止于扫描预算与配置页数。
    /// 只读取列表，不读取详情，也不改变任何发送或领取事实。
    pub async fn scan_sent_since(
        &self,
        world: MoogleMailWorld,
        cutoff_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        maximum_pages: usize,
    ) -> Result<MoogleMailSentRecoveryScan, MoogleMailError> {
        if cutoff_at.timestamp_nanos_opt().is_none()
            || observed_at.timestamp_nanos_opt().is_none()
            || maximum_pages == 0
            || maximum_pages > self.config.maximum_pages
        {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(world, MoogleMailOperation::Sent),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_sent_recovery_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })??;
        let result: Result<MoogleMailSentRecoveryScan, MoogleMailError> =
            tokio::time::timeout_at(deadline, async {
                let round = self.client.read_round(session.cookie(), world, false)?;
                let list =
                    self.client.observe_sent_list(session.cookie(), &round, maximum_pages).await?;
                let observed_at = observed_at.min(round.observed_at());
                // Sent 页面只有分钟精度；含秒 cutoff 必须按所在分钟下界比较，保留整个重叠分钟。
                let cutoff_minute = cutoff_at
                    - chrono::Duration::seconds(i64::from(cutoff_at.second()))
                    - chrono::Duration::nanoseconds(i64::from(cutoff_at.timestamp_subsec_nanos()));
                let covers_cutoff = list.complete
                    || list.summaries.last().is_some_and(|row| row.sent_at < cutoff_minute);
                let entries = list
                    .summaries
                    .iter()
                    .filter(|row| row.sent_at >= cutoff_minute)
                    .cloned()
                    .collect();
                self.client.check_read_round(session.cookie(), &round).await?;
                Ok(MoogleMailSentRecoveryScan {
                    entries,
                    covers_cutoff,
                    pages_scanned: list.pages_scanned,
                    observed_at,
                })
            })
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "scan_sent_recovery_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(world)
            })?;
        match result {
            Ok(scan) => {
                session.check_current().await?;
                Ok(scan)
            }
            Err(error) if error.is_session_expired() => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// 恢复核对：枚举 Read Mail 全部摘要；不含正文，不改变已读状态。
    ///
    /// Read Mail 的列表时间列是远端 `Sent`（原始到达时间），不是本地处理时间。恢复库丢失
    /// 的投递可能“备份前到达、恢复后才处理”，按 `cutoff_at` 过滤恰好会漏掉这类记录，因此
    /// 这里对 Read Mail 不使用时间截断；是否完整只取决于列表是否读到末页（`complete`）。
    pub async fn scan_read_mail_since(
        &self,
        world: MoogleMailWorld,
        _cutoff_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        maximum_pages: usize,
    ) -> Result<MoogleMailInboundRecoveryScan, MoogleMailError> {
        if _cutoff_at.timestamp_nanos_opt().is_none()
            || observed_at.timestamp_nanos_opt().is_none()
            || maximum_pages == 0
            || maximum_pages > self.config.maximum_pages
        {
            return Err(operation_error(
                MoogleMailOperation::ReadMail,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(world, MoogleMailOperation::ReadMail),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_read_mail_recovery_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })??;
        let result: Result<MoogleMailInboundRecoveryScan, MoogleMailError> =
            tokio::time::timeout_at(deadline, async {
                let round = self.client.read_round(session.cookie(), world, false)?;
                let list = self
                    .client
                    .observe_incoming_list(session.cookie(), &round, maximum_pages, true)
                    .await?;
                let observed_at = observed_at.min(round.observed_at());
                // 不做到达时间截断；只有读到末页才能声称 Read Mail 覆盖完整。
                let covers_cutoff = list.complete;
                let entries = list.summaries.clone();
                self.client.check_read_round(session.cookie(), &round).await?;
                Ok(MoogleMailInboundRecoveryScan { entries, covers_cutoff, observed_at })
            })
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "scan_read_mail_recovery_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(world)
            })?;
        match result {
            Ok(scan) => {
                session.check_current().await?;
                Ok(scan)
            }
            Err(error) if error.is_session_expired() => {
                session.invalidate_current().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}
