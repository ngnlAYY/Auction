//! Sent 列表与详情的证据读取：按已知 MID 定位或按收件人/主题/资产核对候选。
//!
//! 列表投影只能筛候选；ID、world、完整收件人、主题与时间必须由详情精确核对，
//! 页数耗尽与多候选都返回不完整结论，绝不生成领取或结算成功证明。

use std::collections::HashSet;

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::mail::mail_recipient_is_list_projection;
use chrono::{DateTime, Utc};

use crate::clients::mooglemail::parser::subject_is_list_projection;
use crate::diagnostics as safe_diagnostics;

use super::super::super::client::MoogleMailReadRound;
use super::super::super::protocol::valid_opaque_id;
use super::error::fetch_budget_exhausted;
use super::*;

impl MoogleMailGateway {
    /// 按已知 MID 定位 Sent 列表行并核对详情；命中不扫无关尾页，判缺失必须到尾页。
    pub async fn read_sent(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        if !valid_opaque_id(external_mail_id) || observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session = self.credentials.begin_read(world, MoogleMailOperation::Sent).await?;
            match self
                .read_sent_with_cookie(
                    world,
                    external_mail_id,
                    observed_at,
                    session.cookie(),
                    false,
                )
                .await
            {
                Ok(snapshot) => {
                    session.check_current().await?;
                    Ok(snapshot)
                }
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_sent_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        // timeout_at 不抢占同步解析，也可能直接接受已就绪的缓存结果。
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }

    /// 在有界页数和绝对截止时间内按收件人、主题和装备剩余资产核对 Sent。
    pub async fn lookup_sent(
        &self,
        request: MoogleMailSentLookupRequest,
    ) -> Result<MoogleMailSentLookupReport, MoogleMailError> {
        if !request.validate(self.config.maximum_pages) {
            return Err(operation_error(
                MoogleMailOperation::Sent,
                request.world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let configured_deadline = std::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(request.world))?;
        let deadline = tokio::time::Instant::from_std(request.deadline.min(configured_deadline));
        let mut session = tokio::time::timeout_at(
            deadline,
            self.credentials.begin_read(request.world, MoogleMailOperation::Sent),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "acquire_sent_session_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })??;
        tokio::time::timeout_at(deadline, async {
            match self.lookup_sent_with_cookie(&request, session.cookie(), deadline).await {
                Ok(report) => {
                    session.check_current().await?;
                    if tokio::time::Instant::now() >= deadline {
                        return Err(fetch_budget_exhausted(request.world));
                    }
                    Ok(report)
                }
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "lookup_sent_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })?
    }

    async fn lookup_sent_with_cookie(
        &self,
        request: &MoogleMailSentLookupRequest,
        cookie: &MoogleMailCookie,
        deadline: tokio::time::Instant,
    ) -> Result<MoogleMailSentLookupReport, MoogleMailError> {
        let round = self.client.read_round(cookie, request.world, false)?;
        let list = tokio::time::timeout_at(
            deadline,
            self.client.observe_sent_list(cookie, &round, request.maximum_pages),
        )
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_sent_list_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(request.world.as_str()),
                    ..Default::default()
                },
            );
            fetch_budget_exhausted(request.world)
        })??;
        let mut candidates = Vec::new();
        for summary in list.summaries.iter().filter(|summary| {
            mail_recipient_is_list_projection(&summary.recipient, &request.recipient)
                && subject_is_list_projection(&summary.subject, &request.subject)
        }) {
            let detail_target =
                self.client.sent_detail(request.world, &summary.external_mail_id)?;
            let document = tokio::time::timeout_at(
                deadline,
                self.client.get_in_round(&detail_target, cookie, &round),
            )
            .await
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "read_sent_detail_timeout",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(request.world.as_str()),
                        ..Default::default()
                    },
                );
                fetch_budget_exhausted(request.world)
            })??;
            let snapshot = self
                .client
                .diagnose_parse(
                    &detail_target,
                    &document,
                    self.parser.parse_sent_detail(
                        &document,
                        summary,
                        request.observed_at.min(round.observed_at()),
                    ),
                )
                .await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(request.world));
            }
            if snapshot.recipient.eq_ignore_ascii_case(&request.recipient)
                && snapshot.subject == request.subject
                && snapshot.remaining_credits == 0
                && snapshot.cod_pending == 0
                && (snapshot.collected
                    || snapshot.remaining_equipment_ids.as_slice()
                        == std::slice::from_ref(&request.external_equipment_id))
            {
                candidates.push(snapshot);
                if candidates.len() >= 2 {
                    break;
                }
            }
        }
        self.client.check_read_round(cookie, &round).await?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(request.world));
        }
        let status = if candidates.len() >= 2 {
            MoogleMailSentLookupStatus::Multiple
        } else if !list.complete {
            MoogleMailSentLookupStatus::Incomplete
        } else {
            match candidates.as_slice() {
                [] => MoogleMailSentLookupStatus::NotObserved,
                [candidate] if candidate.collected => MoogleMailSentLookupStatus::CollectedUnproven,
                _ => MoogleMailSentLookupStatus::Unique,
            }
        };
        Ok(MoogleMailSentLookupReport {
            status,
            candidates,
            pages_scanned: list.pages_scanned,
            scan_complete: list.complete,
            observed_at: request.observed_at.min(round.observed_at()),
        })
    }

    /// 从 `start_page` 起逐页查找已知 MID 的列表行，返回该行及其页码。
    ///
    /// 读到尾页仍未出现返回 `Ok(None)`；页数上限耗尽不是目标不存在，返回不完整错误，
    /// 不得生成领取或结算成功证明。每次调用使用独立的去重集合：回退重扫时前一遍见过的
    /// 行可能已经移到别的页，不能被当成重复行跳过。
    async fn locate_sent_row(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        cookie: &MoogleMailCookie,
        round: &MoogleMailReadRound,
        start_page: usize,
        deadline: tokio::time::Instant,
    ) -> Result<Option<(MoogleMailSentSummary, usize)>, MoogleMailError> {
        let mut seen = HashSet::new();
        for page_index in start_page..self.config.maximum_pages {
            let target = self.client.sent_page(world, page_index)?;
            let document = self.client.get_in_round(&target, cookie, round).await?;
            let page = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.scan_sent_page(&document, world, page_index, &mut seen),
                )
                .await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(world));
            }
            if let Some(found) =
                page.summaries.into_iter().find(|entry| entry.external_mail_id == external_mail_id)
            {
                return Ok(Some((found, page_index)));
            }
            if !page.has_next_page {
                return Ok(None);
            }
        }
        Err(operation_error(
            MoogleMailOperation::Sent,
            world,
            MoogleMailErrorCategory::SentScanPageLimit,
        ))
    }

    pub(in super::super) async fn read_sent_with_cookie(
        &self,
        world: MoogleMailWorld,
        external_mail_id: &str,
        observed_at: DateTime<Utc>,
        cookie: &MoogleMailCookie,
        fresh: bool,
    ) -> Result<MoogleMailSentSnapshot, MoogleMailError> {
        // 履约证据不能早于 claim；普通已知 MID 读取使用新鲜私有轮次，不借用旧缓存时间。
        // 资金组合读取仍沿用原 fresh 写屏障及独占 session，不能把私有只读轮次当作资产授权。
        let round = if fresh {
            self.client.read_round(cookie, world, true)?
        } else {
            self.client.isolated_read_round(cookie, world)?
        };
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        tokio::time::timeout_at(deadline, async {
            // 已知 MID 的正向列表身份证明不依赖无关尾页；每次履约观察保持独立新鲜。
            // 这不是新发送的基线/候选搜索，不能据此缩短后者的完整扫描或放宽资金证据期限。
            // 先从上次命中页读起；未命中再从第 0 页完整重扫，缺失结论只来自完整扫描。
            let hinted = match self
                .sent_page_hints
                .start_page(world, external_mail_id)
                .filter(|page| (1..self.config.maximum_pages).contains(page))
            {
                Some(start) => match self
                    .locate_sent_row(world, external_mail_id, cookie, &round, start, deadline)
                    .await
                {
                    Ok(located) => located,
                    // 列表变短后提示页可能越界；页面不符时按无提示处理，结论交给完整扫描。
                    Err(error) if error.category == MoogleMailErrorCategory::ProtocolDrift => None,
                    Err(error) => return Err(error),
                },
                None => None,
            };
            let located = match hinted {
                Some(located) => Some(located),
                None => {
                    self.locate_sent_row(world, external_mail_id, cookie, &round, 0, deadline)
                        .await?
                }
            };
            let Some((summary, page_index)) = located else {
                self.sent_page_hints.forget(world, external_mail_id);
                return Err(operation_error(
                    MoogleMailOperation::Sent,
                    world,
                    MoogleMailErrorCategory::NotObserved,
                ));
            };
            self.sent_page_hints.record(world, external_mail_id, page_index);
            let target = self.client.sent_detail(world, external_mail_id)?;
            let document = self.client.get_in_round(&target, cookie, &round).await?;
            let snapshot = self
                .client
                .diagnose_parse(
                    &target,
                    &document,
                    self.parser.parse_sent_detail(
                        &document,
                        &summary,
                        observed_at.min(round.observed_at()),
                    ),
                )
                .await?;
            if snapshot.external_mail_id != external_mail_id
                || snapshot.world != world
                || !mail_recipient_is_list_projection(&summary.recipient, &snapshot.recipient)
                || !subject_is_list_projection(&summary.subject, &snapshot.subject)
                || snapshot.sent_at != summary.sent_at
            {
                return self
                    .client
                    .diagnose_parse(
                        &target,
                        &document,
                        Err(operation_error(
                            MoogleMailOperation::SentDetail,
                            world,
                            MoogleMailErrorCategory::IdentityMismatch,
                        )),
                    )
                    .await;
            }
            self.client.check_read_round(cookie, &round).await?;
            if tokio::time::Instant::now() >= deadline {
                return Err(fetch_budget_exhausted(world));
            }
            Ok(snapshot)
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "build_sent_snapshot_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?
    }
}
