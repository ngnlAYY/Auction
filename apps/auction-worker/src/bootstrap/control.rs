//! Worker control socket、后台任务回收与 heartbeat 辅助装配。

use super::*;

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

#[cfg(feature = "postgres")]
struct WorkerHeartbeatHealth {
    database: Arc<dyn HeartbeatHealthReader>,
    control: WorkerControlHandle,
}

#[cfg(feature = "postgres")]
impl HeartbeatHealthReader for WorkerHeartbeatHealth {
    fn readiness(&self) -> auction_observability::HeartbeatReadinessFuture<'_> {
        Box::pin(async move { self.database.readiness().await && self.control.is_ready() })
    }

    fn audit_failure_count(&self) -> i64 {
        self.database.audit_failure_count()
    }
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
pub(super) fn abort_worker_task(task: &Option<TaskGuard<()>>) {
    if let Some(task) = task {
        task.abort();
    }
}

#[cfg(feature = "postgres")]
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

#[cfg(feature = "postgres")]
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

/// W1.10：Worker 侧远端只读执行器。
/// 复用本进程唯一的装备来源适配器与按世界的 MoogleMail gateway，因此 Admin 装备读取与
/// CLI Sent 核查和 Worker 自身请求共用同一请求门、账号熔断与 W1.9 暂停判定；
/// 暂停只阻止新请求，已进入 sending 围栏的写入不受影响。
#[cfg(feature = "postgres")]
pub(super) struct WorkerRemoteReads {
    mail: super::mail::MailControlRemoteReads,
    switches: auction_application::RuntimeSwitches,
}

#[cfg(feature = "postgres")]
impl WorkerRemoteReads {
    /// 装配 control 远端读取能力；未配置 MoogleMail 时不装配，路由明确返回能力不可用。
    pub(super) fn build(
        mail: super::mail::MailControlRemoteReads,
        switches: auction_application::RuntimeSwitches,
    ) -> Arc<dyn auction_application::WorkerRemoteReadPort> {
        Arc::new(Self { mail, switches })
    }
}

#[cfg(feature = "postgres")]
impl auction_application::WorkerRemoteReadPort for WorkerRemoteReads {
    fn read_equipment(
        &self,
        request: auction_application::WorkerEquipmentReadRequest,
    ) -> auction_application::WorkerRemoteReadFuture<
        '_,
        auction_application::WorkerEquipmentReadResponse,
    > {
        Box::pin(async move {
            if self.switches.external_access_paused() {
                return Err(auction_application::WorkerRemoteReadError::Paused);
            }
            let snapshot = auction_application::AdminIntakeSourceReader::read_url(
                self.mail.equipment.as_ref(),
                request.world_id,
                request.equipment_url,
            )
            .await
            .map_err(|error| match error {
                auction_application::ApplicationError::AdminIntake(error) => {
                    remote_read_intake_error(error)
                }
                _ => auction_application::WorkerRemoteReadError::Unavailable,
            })?;
            auction_application::WorkerEquipmentReadResponse::from_snapshot(snapshot)
        })
    }

    fn lookup_sent(
        &self,
        request: auction_application::WorkerSentLookupRequest,
    ) -> auction_application::WorkerRemoteReadFuture<
        '_,
        auction_application::WorkerSentLookupResponse,
    > {
        Box::pin(async move {
            if self.switches.external_access_paused() {
                return Err(auction_application::WorkerRemoteReadError::Paused);
            }
            let world = MoogleMailWorld::parse(&request.world)
                .ok_or(auction_application::WorkerRemoteReadError::Invalid)?;
            let gateway = self
                .mail
                .gateways
                .get(&world)
                .ok_or(auction_application::WorkerRemoteReadError::Unavailable)?;
            let report = gateway
                .lookup_sent(MoogleMailSentLookupRequest {
                    world,
                    recipient: request.recipient,
                    subject: request.subject,
                    external_equipment_id: request.external_equipment_id,
                    maximum_pages: self.mail.maximum_pages,
                    deadline: std::time::Instant::now() + self.mail.fetch_timeout,
                    observed_at: Utc::now(),
                })
                .await
                .map_err(|error| match error.category {
                    MoogleMailErrorCategory::Configuration => {
                        auction_application::WorkerRemoteReadError::Invalid
                    }
                    MoogleMailErrorCategory::SentScanTimeout => {
                        auction_application::WorkerRemoteReadError::Timeout
                    }
                    _ => auction_application::WorkerRemoteReadError::Unavailable,
                })?;
            Ok(auction_application::WorkerSentLookupResponse {
                status: report.status.label().to_owned(),
                scan_complete: report.scan_complete,
                pages_scanned: u32::try_from(report.pages_scanned).unwrap_or(u32::MAX),
                observed_at: report.observed_at,
                candidates: report
                    .candidates
                    .into_iter()
                    .map(|candidate| auction_application::WorkerSentLookupCandidate {
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
}

#[cfg(feature = "postgres")]
fn remote_read_intake_error(
    error: auction_application::AdminIntakeError,
) -> auction_application::WorkerRemoteReadError {
    match error {
        auction_application::AdminIntakeError::ExternalAccessPaused => {
            auction_application::WorkerRemoteReadError::Paused
        }
        auction_application::AdminIntakeError::InvalidCommand
        | auction_application::AdminIntakeError::InvalidSource => {
            auction_application::WorkerRemoteReadError::Invalid
        }
        auction_application::AdminIntakeError::SourceUnavailable
        | auction_application::AdminIntakeError::SourceUnconfigured
        | auction_application::AdminIntakeError::KeyUnavailable
        | auction_application::AdminIntakeError::CommandConflict
        | auction_application::AdminIntakeError::StoredResultCorrupt => {
            auction_application::WorkerRemoteReadError::Unavailable
        }
    }
}
