//! Worker 侧装备、Sent、收件箱与论坛只读控制能力。

use std::sync::Arc;

use auction_domain::SettlementWorld;
use auction_integrations::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailSentLookupRequest,
    MoogleMailWorld,
};
use auction_time::Timestamp;

use super::super::mail::MailControlRemoteReads;
/// Worker 侧远端只读执行器。
/// 复用本进程唯一的装备来源适配器与按世界的 MoogleMail gateway，因此 Admin 装备读取、
/// CLI Sent 核查与 Worker 自身请求共用同一在途锁、账号熔断与暂停判定；Sent 核查另走邮件
/// 请求门，装备页只按装备来源策略的最小间隔节流。
/// 暂停只阻止新请求，已进入 sending 围栏的写入不受影响。
pub(in crate::bootstrap) struct WorkerRemoteReads {
    mail: Option<MailControlRemoteReads>,
    /// 与论坛回帖循环共用的只读主题读取器；未启用论坛时恢复扫描只报告能力不可用。
    forum: Option<Arc<dyn auction_application::forum::ForumTopicReader>>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
}

impl WorkerRemoteReads {
    /// 装配 control 远端读取能力；任一外部读取缺失时只关闭对应能力。
    pub(in crate::bootstrap) fn build(
        mail: Option<MailControlRemoteReads>,
        forum: Option<Arc<dyn auction_application::forum::ForumTopicReader>>,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Arc<dyn auction_application::operations::WorkerRemoteReadPort> {
        Arc::new(Self { mail, forum, switches })
    }

    fn mail(
        &self,
    ) -> Result<&MailControlRemoteReads, auction_application::operations::WorkerRemoteReadError>
    {
        self.mail
            .as_ref()
            .ok_or(auction_application::operations::WorkerRemoteReadError::Unavailable)
    }

    fn gateway(
        &self,
        world: MoogleMailWorld,
    ) -> Result<&Arc<MoogleMailGateway>, auction_application::operations::WorkerRemoteReadError>
    {
        self.mail()?
            .gateways
            .get(&world)
            .ok_or(auction_application::operations::WorkerRemoteReadError::Unavailable)
    }
}

impl auction_application::operations::WorkerRemoteReadPort for WorkerRemoteReads {
    fn read_equipment(
        &self,
        request: auction_application::operations::WorkerEquipmentReadRequest,
    ) -> auction_application::operations::WorkerRemoteReadFuture<
        '_,
        auction_application::operations::WorkerEquipmentReadResponse,
    > {
        Box::pin(async move {
            if self.switches.external_access_paused() {
                return Err(auction_application::operations::WorkerRemoteReadError::Paused);
            }
            let mail = self.mail()?;
            let snapshot = auction_application::admin::AdminIntakeSourceReader::read_url(
                mail.equipment.as_ref(),
                request.world_id,
                request.equipment_url,
            )
            .await
            .map_err(|error| match error {
                auction_application::core::ApplicationError::AdminIntake(error) => {
                    remote_read_intake_error(error)
                }
                _ => auction_application::operations::WorkerRemoteReadError::Unavailable,
            })?;
            auction_application::operations::WorkerEquipmentReadResponse::from_snapshot(snapshot)
        })
    }

    fn lookup_sent(
        &self,
        request: auction_application::operations::WorkerSentLookupRequest,
    ) -> auction_application::operations::WorkerRemoteReadFuture<
        '_,
        auction_application::operations::WorkerSentLookupResponse,
    > {
        Box::pin(async move {
            if self.switches.external_access_paused() {
                return Err(auction_application::operations::WorkerRemoteReadError::Paused);
            }
            if !request.validate() {
                return Err(auction_application::operations::WorkerRemoteReadError::Invalid);
            }
            let requested_world = request.world.clone();
            let world = MoogleMailWorld::parse(&request.world)
                .ok_or(auction_application::operations::WorkerRemoteReadError::Invalid)?;
            let mail = self.mail()?;
            let gateway = self.gateway(world)?;
            let report = gateway
                .lookup_sent(MoogleMailSentLookupRequest {
                    world,
                    recipient: request.recipient,
                    subject: request.subject,
                    external_equipment_id: request.external_equipment_id,
                    maximum_pages: mail.maximum_pages.min(gateway.maximum_pages()),
                    deadline: std::time::Instant::now() + mail.fetch_timeout,
                    observed_at: Timestamp::now(),
                })
                .await
                .map_err(|error| match error.category {
                    MoogleMailErrorCategory::Configuration => {
                        auction_application::operations::WorkerRemoteReadError::Invalid
                    }
                    MoogleMailErrorCategory::SentScanTimeout => {
                        auction_application::operations::WorkerRemoteReadError::Timeout
                    }
                    _ => auction_application::operations::WorkerRemoteReadError::Unavailable,
                })?;
            let response = auction_application::operations::WorkerSentLookupResponse {
                status: report.status.label().to_owned(),
                scan_complete: report.scan_complete,
                pages_scanned: u32::try_from(report.pages_scanned).unwrap_or(u32::MAX),
                observed_at: report.observed_at,
                candidates: report
                    .candidates
                    .into_iter()
                    .map(|candidate| auction_application::operations::WorkerSentLookupCandidate {
                        world: requested_world.clone(),
                        external_mail_id: candidate.external_mail_id,
                        recipient: candidate.recipient,
                        subject: candidate.subject,
                        sent_at: Some(candidate.sent_at),
                        remaining_credits: candidate.remaining_credits,
                        cod_pending: candidate.cod_pending,
                        remaining_equipment_ids: candidate.remaining_equipment_ids,
                        collected: candidate.collected,
                        observed_at: candidate.observed_at,
                    })
                    .collect(),
            };
            if !response.validate_for_world(&requested_world) {
                return Err(auction_application::operations::WorkerRemoteReadError::Incompatible);
            }
            Ok(response)
        })
    }

    /// 暂停状态下的一次有界只读扫描。
    /// 暂停是前置条件：未暂停时拒绝执行，防止恢复核对与正常 Worker 流量交叉读取。
    /// 只读取列表/详情与论坛主题，不执行 send、take、mark-read 或任何本地写入。
    fn recovery_scan(
        &self,
        request: auction_application::operations::WorkerRecoveryScanRequest,
    ) -> auction_application::operations::WorkerRemoteReadFuture<
        '_,
        auction_application::operations::WorkerRecoveryScanResponse,
    > {
        Box::pin(async move {
            if !request.validate() {
                return Err(auction_application::operations::WorkerRemoteReadError::Invalid);
            }
            if !self.switches.external_access_paused() {
                return Err(auction_application::operations::WorkerRemoteReadError::NotPaused);
            }
            let observed_at = Timestamp::now();
            let mut response = auction_application::operations::WorkerRecoveryScanResponse {
                status: "complete".to_owned(),
                observed_at,
                paused: true,
                sent_complete: true,
                read_mail_complete: true,
                pages_scanned: 0,
                errors: Vec::new(),
                sent: Vec::new(),
                read_mail: Vec::new(),
                sent_details: Vec::new(),
                forum: Vec::new(),
            };
            let maximum_pages = request.maximum_pages as usize;
            let maximum_entries = auction_application::operations::MAX_RECOVERY_SCAN_ENTRIES;

            if request.include_sent {
                for world_id in &request.sent_worlds {
                    if !self.switches.external_access_paused() {
                        response.errors.push("external_access_resumed".to_owned());
                        response.sent_complete = false;
                        break;
                    }
                    let Some(world) = MoogleMailWorld::parse(world_id) else {
                        response.errors.push("sent_world_invalid".to_owned());
                        response.sent_complete = false;
                        continue;
                    };
                    let Some(gateway) =
                        self.mail.as_ref().and_then(|mail| mail.gateways.get(&world))
                    else {
                        response.errors.push("sent_capability_unavailable".to_owned());
                        response.sent_complete = false;
                        continue;
                    };
                    // 请求页数是配置上限内的子集；gateway 仍按自身硬上限校验。
                    let pages = maximum_pages.min(gateway.maximum_pages());
                    match gateway
                        .scan_sent_since(world, request.cutoff_at, observed_at, pages)
                        .await
                    {
                        Ok(scan) => {
                            response.pages_scanned = response.pages_scanned.saturating_add(
                                u32::try_from(scan.pages_scanned).unwrap_or(u32::MAX),
                            );
                            response.sent_complete &= scan.covers_cutoff;
                            let mut entries = scan.entries.into_iter();
                            let remaining = maximum_entries.saturating_sub(response.sent.len());
                            response.sent.extend(entries.by_ref().take(remaining).map(|entry| {
                                auction_application::operations::WorkerRecoverySentEntry {
                                    world: world_id.clone(),
                                    external_mail_id: entry.external_mail_id,
                                    recipient: entry.recipient,
                                    subject: entry.subject,
                                    sent_at: entry.sent_at,
                                }
                            }));
                            if entries.next().is_some() {
                                response.sent_complete = false;
                                response.errors.push("recovery_scan_truncated".to_owned());
                            }
                        }
                        Err(error) => {
                            response.sent_complete = false;
                            response.errors.push(sent_scan_error_code(&error));
                        }
                    }
                }
            }

            if request.include_read_mail {
                for world_id in &request.read_mail_worlds {
                    if !self.switches.external_access_paused() {
                        response.errors.push("external_access_resumed".to_owned());
                        response.read_mail_complete = false;
                        break;
                    }
                    let Some(world) = MoogleMailWorld::parse(world_id) else {
                        response.errors.push("read_mail_world_invalid".to_owned());
                        response.read_mail_complete = false;
                        continue;
                    };
                    let Some(gateway) =
                        self.mail.as_ref().and_then(|mail| mail.gateways.get(&world))
                    else {
                        response.errors.push("read_mail_capability_unavailable".to_owned());
                        response.read_mail_complete = false;
                        continue;
                    };
                    let pages = maximum_pages.min(gateway.maximum_pages());
                    match gateway
                        .scan_read_mail_since(world, request.cutoff_at, observed_at, pages)
                        .await
                    {
                        Ok(scan) => {
                            response.read_mail_complete &= scan.covers_cutoff;
                            let mut entries = scan.entries.into_iter();
                            let remaining =
                                maximum_entries.saturating_sub(response.read_mail.len());
                            response.read_mail.extend(entries.by_ref().take(remaining).map(
                                |entry| {
                                    auction_application::operations::WorkerRecoveryInboundEntry {
                                        world: world_id.clone(),
                                        external_mail_id: entry.external_mail_id,
                                        sender: entry.sender,
                                        subject: entry.subject,
                                        received_at: entry.received_at,
                                    }
                                },
                            ));
                            if entries.next().is_some() {
                                response.read_mail_complete = false;
                                response.errors.push("recovery_scan_truncated".to_owned());
                            }
                        }
                        Err(error) => {
                            response.read_mail_complete = false;
                            response.errors.push(read_mail_scan_error_code(&error));
                        }
                    }
                }
            }

            for detail in &request.sent_details {
                if response.sent_details.len() >= maximum_entries {
                    response.sent_complete = false;
                    response.errors.push("recovery_scan_truncated".to_owned());
                    break;
                }
                if !self.switches.external_access_paused() {
                    response.sent_complete = false;
                    response.errors.push("external_access_resumed".to_owned());
                    break;
                }
                let Some(world) = MoogleMailWorld::parse(&detail.world) else {
                    response.sent_complete = false;
                    response.errors.push("sent_detail_world_invalid".to_owned());
                    continue;
                };
                let Some(gateway) = self.mail.as_ref().and_then(|mail| mail.gateways.get(&world))
                else {
                    response.sent_complete = false;
                    response.errors.push("sent_capability_unavailable".to_owned());
                    continue;
                };
                match gateway.read_sent(world, &detail.external_mail_id, observed_at).await {
                    Ok(snapshot) => {
                        response.sent_details.push(
                            auction_application::operations::WorkerSentLookupCandidate {
                                world: detail.world.clone(),
                                external_mail_id: snapshot.external_mail_id,
                                recipient: snapshot.recipient,
                                subject: snapshot.subject,
                                sent_at: Some(snapshot.sent_at),
                                remaining_credits: snapshot.remaining_credits,
                                cod_pending: snapshot.cod_pending,
                                remaining_equipment_ids: snapshot.remaining_equipment_ids,
                                collected: snapshot.collected,
                                observed_at: snapshot.observed_at,
                            },
                        );
                    }
                    Err(error) => {
                        response.sent_complete = false;
                        response.errors.push(sent_detail_error_code(&error));
                    }
                }
            }

            if request.include_forum {
                // 没有目标时不需要读取论坛；未配置能力但有目标时明确报告不可用。
                if let Some(reader) = self.forum.as_ref() {
                    'forum_targets: for target in &request.forum_threads {
                        if !self.switches.external_access_paused() {
                            response.errors.push("external_access_resumed".to_owned());
                            break;
                        }
                        let Some(world) = SettlementWorld::from_name(&target.world) else {
                            response.errors.push("forum_world_invalid".to_owned());
                            continue;
                        };
                        match reader
                            .fetch_topic_since(
                                world,
                                target.thread_id.clone(),
                                None,
                                target.after_post_number,
                            )
                            .await
                        {
                            Ok(scan) => {
                                if scan.validate().is_err() {
                                    response.errors.push("forum_scan_invalid".to_owned());
                                    continue;
                                }
                                match (scan.page_count, scan.page_limit) {
                                    (Some(page_count), Some(page_limit))
                                        if page_count > page_limit =>
                                    {
                                        response.errors.push("forum_scan_truncated".to_owned());
                                    }
                                    (Some(_), Some(_)) => {}
                                    (None, None) => response
                                        .errors
                                        .push("forum_scan_incomplete_metadata".to_owned()),
                                    _ => response
                                        .errors
                                        .push("forum_scan_incomplete_metadata".to_owned()),
                                }
                                // 固定楼层页（目录续页与系统日志）在前、用户回帖在后；
                                // `ForumTopicScan::validate` 保证与预览帖子等长且一一对应。
                                for (post_number, post) in scan
                                    .fixed_post_numbers
                                    .iter()
                                    .chain(scan.post_numbers.iter())
                                    .zip(&scan.preview.posts)
                                {
                                    if *post_number > target.after_post_number {
                                        if response.forum.len() >= maximum_entries {
                                            response
                                                .errors
                                                .push("recovery_scan_truncated".to_owned());
                                            break 'forum_targets;
                                        }
                                        response.forum.push(
                                            auction_application::operations::WorkerRecoveryForumPost {
                                                world: target.world.clone(),
                                                thread_id: target.thread_id.clone(),
                                                post_id: post.post.id.clone(),
                                                post_number: *post_number,
                                            },
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                tracing::warn!(
                                    kind = error.safe_kind(),
                                    phase = "recovery_forum_scan",
                                    "恢复核对读取论坛主题失败"
                                );
                                response.errors.push("forum_scan_failed".to_owned());
                            }
                        }
                    }
                } else if !request.forum_threads.is_empty() {
                    response.errors.push("forum_capability_unavailable".to_owned());
                }
            }

            // 截断保护同时覆盖逐项入列和最终投影，避免每类结果集合无界增长。
            if response.sent.len() > maximum_entries {
                response.sent.truncate(maximum_entries);
                response.sent_complete = false;
            }
            if response.read_mail.len() > maximum_entries {
                response.read_mail.truncate(maximum_entries);
                response.read_mail_complete = false;
            }
            if response.sent_details.len() > maximum_entries {
                response.sent_details.truncate(maximum_entries);
                response.sent_complete = false;
            }
            if response.forum.len() > maximum_entries {
                response.forum.truncate(maximum_entries);
                response.errors.push("recovery_scan_truncated".to_owned());
            }
            response.paused = self.switches.external_access_paused();
            if !response.paused {
                response.errors.push("external_access_resumed".to_owned());
            }
            response.errors.sort();
            response.errors.dedup();
            // The response contract caps error codes at 16; preserve partial status and flag omitted detail.
            if response.errors.len() > 16 {
                response.errors.truncate(15);
                response.errors.push("recovery_scan_truncated".to_owned());
                response.errors.sort();
                response.errors.dedup();
            }
            if !response.paused || !response.errors.is_empty() {
                response.status = "partial".to_owned();
            } else if !response.sent_complete || !response.read_mail_complete {
                response.status = "truncated".to_owned();
            }
            if !response.validate() {
                return Err(auction_application::operations::WorkerRemoteReadError::Incompatible);
            }
            Ok(response)
        })
    }
}

fn sent_scan_error_code(error: &MoogleMailError) -> String {
    match error.category {
        MoogleMailErrorCategory::Configuration => "sent_scan_configuration".to_owned(),
        MoogleMailErrorCategory::SentScanTimeout => "sent_scan_timeout".to_owned(),
        _ => "sent_scan_failed".to_owned(),
    }
}

fn read_mail_scan_error_code(error: &MoogleMailError) -> String {
    match error.category {
        MoogleMailErrorCategory::Configuration => "read_mail_scan_configuration".to_owned(),
        MoogleMailErrorCategory::SentScanTimeout => "read_mail_scan_timeout".to_owned(),
        _ => "read_mail_scan_failed".to_owned(),
    }
}

fn sent_detail_error_code(error: &MoogleMailError) -> String {
    match error.category {
        MoogleMailErrorCategory::Configuration => "sent_detail_configuration".to_owned(),
        MoogleMailErrorCategory::SentScanTimeout => "sent_detail_timeout".to_owned(),
        _ => "sent_detail_failed".to_owned(),
    }
}

fn remote_read_intake_error(
    error: auction_application::admin::AdminIntakeError,
) -> auction_application::operations::WorkerRemoteReadError {
    match error {
        auction_application::admin::AdminIntakeError::ExternalAccessPaused => {
            auction_application::operations::WorkerRemoteReadError::Paused
        }
        auction_application::admin::AdminIntakeError::InvalidCommand
        | auction_application::admin::AdminIntakeError::InvalidSource => {
            auction_application::operations::WorkerRemoteReadError::Invalid
        }
        auction_application::admin::AdminIntakeError::SourceUnavailable
        | auction_application::admin::AdminIntakeError::SourceUnconfigured
        | auction_application::admin::AdminIntakeError::KeyUnavailable
        | auction_application::admin::AdminIntakeError::CommandConflict
        | auction_application::admin::AdminIntakeError::StoredResultCorrupt => {
            auction_application::operations::WorkerRemoteReadError::Unavailable
        }
    }
}
