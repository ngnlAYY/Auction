//! Worker control socket、后台任务回收与 heartbeat 辅助装配。

use std::sync::Arc;

use anyhow::Context;
use auction_config::AppConfig;
use auction_domain::SettlementWorld;
use auction_integrations::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailSentLookupRequest,
    MoogleMailWorld,
};
use auction_jobs::{WorkerControlHandle, WorkerControlServerConfig};
use auction_observability::{HeartbeatHealthReader, HeartbeatPublisher};
use auction_process::TaskGuard;
use auction_process::join_error_kind;
use chrono::Utc;

/// 常驻任务退出即撤销 readiness；panic、abort 和首轮 poll 前取消也必须触发同一收口。
#[cfg(unix)]
pub(super) struct WorkerRuntimeTaskExit {
    control: WorkerControlHandle,
    runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
    reason: &'static str,
}

#[cfg(unix)]
impl WorkerRuntimeTaskExit {
    pub(super) fn new(
        control: WorkerControlHandle,
        runtime_failure: tokio::sync::mpsc::Sender<&'static str>,
        reason: &'static str,
    ) -> Self {
        Self { control, runtime_failure, reason }
    }
}

#[cfg(unix)]
impl Drop for WorkerRuntimeTaskExit {
    fn drop(&mut self) {
        self.control.mark_draining();
        let _ = self.runtime_failure.try_send(self.reason);
    }
}

struct WorkerHeartbeatHealth {
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
}

impl HeartbeatHealthReader for WorkerHeartbeatHealth {
    fn readiness(&self) -> auction_observability::HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.control.is_ready() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

pub(super) fn worker_heartbeat_health(
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
) -> Arc<dyn HeartbeatHealthReader> {
    Arc::new(WorkerHeartbeatHealth { database, control })
}

#[cfg(unix)]
pub(super) fn worker_control_server_config(
    config: &AppConfig,
) -> anyhow::Result<WorkerControlServerConfig> {
    let control = &config.worker.control;
    WorkerControlServerConfig::new(
        control.socket_path.clone(),
        control.runtime_directory.clone(),
        control.group.clone(),
        control.allowed_peer_uids.clone(),
        control.allowed_peer_gids.clone(),
    )
    .map_err(|error| anyhow::Error::msg(error.to_string()))
}

pub(super) fn abort_worker_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

pub(super) async fn join_worker_task(task: &mut Option<TaskGuard<()>>, name: &'static str) {
    let Some(task) = task else {
        return;
    };
    if let Some(Err(error)) = task.join().await
        && !error.is_cancelled()
    {
        let kind = join_error_kind(&error);
        tracing::warn!(task = name, kind, "worker background task join failed");
    }
}

pub(super) async fn join_worker_control_task<E>(
    task: &mut Option<TaskGuard<Result<(), E>>>,
    name: &'static str,
) -> anyhow::Result<()>
where
    E: std::fmt::Display,
{
    let Some(task) = task else {
        return Ok(());
    };
    match task.join().await {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(error))) => {
            Err(anyhow::Error::msg(error.to_string())).with_context(|| format!("run {name}"))
        }
        Some(Err(error)) => Err(error).with_context(|| format!("join {name}")),
        None => Ok(()),
    }
}

pub(super) fn spawn_heartbeat(publisher: Arc<HeartbeatPublisher>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10));
        // 心跳只描述当前状态；数据库恢复后跳过漏掉的 tick，避免集中重复发布。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = publisher.publish_current().await {
                tracing::warn!(error = %error, "worker heartbeat publish failed");
            }
        }
    })
}

/// 只在 Worker readiness 正常时向外部服务报到；地址只存在于敏感配置与闭包内。
pub(super) fn spawn_external_heartbeat(
    publisher: Arc<HeartbeatPublisher>,
    address: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if !publisher.external_heartbeat_ready() {
                continue;
            }
            let result = auction_integrations::send_heartbeat(Some(address.as_str())).await;
            match result.status {
                auction_integrations::BackupHeartbeatStatus::Sent => {
                    tracing::debug!("worker external heartbeat sent");
                }
                auction_integrations::BackupHeartbeatStatus::Failed => {
                    tracing::warn!(
                        http_status = result.http_status,
                        "worker external heartbeat failed"
                    );
                }
                auction_integrations::BackupHeartbeatStatus::NotConfigured => return,
            }
        }
    })
}

/// Worker 侧远端只读执行器。
/// 复用本进程唯一的装备来源适配器与按世界的 MoogleMail gateway，因此 Admin 装备读取与
/// CLI Sent 核查和 Worker 自身请求共用同一请求门、账号熔断与暂停判定；
/// 暂停只阻止新请求，已进入 sending 围栏的写入不受影响。
pub(super) struct WorkerRemoteReads {
    mail: Option<super::mail::MailControlRemoteReads>,
    /// 与论坛回帖循环共用的只读主题读取器；未启用论坛时恢复扫描只报告能力不可用。
    forum: Option<Arc<dyn auction_application::forum::ForumTopicReader>>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
}

impl WorkerRemoteReads {
    /// 装配 control 远端读取能力；任一外部读取缺失时只关闭对应能力。
    pub(super) fn build(
        mail: Option<super::mail::MailControlRemoteReads>,
        forum: Option<Arc<dyn auction_application::forum::ForumTopicReader>>,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Arc<dyn auction_application::operations::WorkerRemoteReadPort> {
        Arc::new(Self { mail, forum, switches })
    }

    fn mail(
        &self,
    ) -> Result<
        &super::mail::MailControlRemoteReads,
        auction_application::operations::WorkerRemoteReadError,
    > {
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
                    maximum_pages: mail.maximum_pages,
                    deadline: std::time::Instant::now() + mail.fetch_timeout,
                    observed_at: Utc::now(),
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
            Ok(auction_application::operations::WorkerSentLookupResponse {
                status: report.status.label().to_owned(),
                scan_complete: report.scan_complete,
                pages_scanned: u32::try_from(report.pages_scanned).unwrap_or(u32::MAX),
                observed_at: report.observed_at,
                candidates: report
                    .candidates
                    .into_iter()
                    .map(|candidate| auction_application::operations::WorkerSentLookupCandidate {
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
            })
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
            let mut response = auction_application::operations::WorkerRecoveryScanResponse {
                status: "complete".to_owned(),
                observed_at: Utc::now(),
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
            let observed_at = response.observed_at;
            let maximum_pages = usize::try_from(request.maximum_pages).unwrap_or(usize::MAX);

            if request.include_sent {
                for world_id in &request.sent_worlds {
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
                            response.sent.extend(scan.entries.into_iter().map(|entry| {
                                auction_application::operations::WorkerRecoverySentEntry {
                                    world: world_id.clone(),
                                    external_mail_id: entry.external_mail_id,
                                    recipient: entry.recipient,
                                    subject: entry.subject,
                                    sent_at: entry.sent_at,
                                }
                            }));
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
                            response.read_mail.extend(scan.entries.into_iter().map(|entry| {
                                auction_application::operations::WorkerRecoveryInboundEntry {
                                    world: world_id.clone(),
                                    external_mail_id: entry.external_mail_id,
                                    sender: entry.sender,
                                    subject: entry.subject,
                                    received_at: entry.received_at,
                                }
                            }));
                        }
                        Err(error) => {
                            response.read_mail_complete = false;
                            response.errors.push(read_mail_scan_error_code(&error));
                        }
                    }
                }
            }

            for detail in &request.sent_details {
                let Some(world) = MoogleMailWorld::parse(&detail.world) else {
                    response.errors.push("sent_detail_world_invalid".to_owned());
                    continue;
                };
                let Some(gateway) = self.mail.as_ref().and_then(|mail| mail.gateways.get(&world))
                else {
                    response.errors.push("sent_capability_unavailable".to_owned());
                    continue;
                };
                match gateway.read_sent(world, &detail.external_mail_id, observed_at).await {
                    Ok(snapshot) => {
                        response.sent_details.push(
                            auction_application::operations::WorkerSentLookupCandidate {
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
                    Err(error) => response.errors.push(sent_detail_error_code(&error)),
                }
            }

            if request.include_forum {
                // 没有目标时不需要读取论坛；未配置能力但有目标时明确报告不可用。
                if let Some(reader) = self.forum.as_ref() {
                    for target in &request.forum_threads {
                        let Some(world) = SettlementWorld::from_name(&target.world) else {
                            response.errors.push("forum_world_invalid".to_owned());
                            continue;
                        };
                        match reader.fetch_topic(world, target.thread_id.clone()).await {
                            Ok(preview) => {
                                for (index, post) in preview.posts.iter().enumerate() {
                                    let post_number =
                                        u32::try_from(index).unwrap_or(u32::MAX).saturating_add(2);
                                    if post_number > target.after_post_number {
                                        response.forum.push(
                                            auction_application::operations::WorkerRecoveryForumPost {
                                                world: target.world.clone(),
                                                thread_id: target.thread_id.clone(),
                                                post_id: post.post.id.clone(),
                                                post_number,
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

            // 截断不会改变任何隔离判断：调用方只使用已返回的条目，并在回执记录不完整。
            if response.sent.len() > auction_application::operations::MAX_RECOVERY_SCAN_ENTRIES {
                response.sent.truncate(auction_application::operations::MAX_RECOVERY_SCAN_ENTRIES);
                response.sent_complete = false;
            }
            if response.read_mail.len() > auction_application::operations::MAX_RECOVERY_SCAN_ENTRIES
            {
                response
                    .read_mail
                    .truncate(auction_application::operations::MAX_RECOVERY_SCAN_ENTRIES);
                response.read_mail_complete = false;
            }
            response.errors.sort();
            response.errors.dedup();
            response.paused = self.switches.external_access_paused();
            if !response.paused {
                response.status = "partial".to_owned();
                response.errors.push("external_access_resumed".to_owned());
            } else if !response.errors.is_empty() {
                response.status = "partial".to_owned();
            } else if !response.sent_complete || !response.read_mail_complete {
                response.status = "truncated".to_owned();
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
