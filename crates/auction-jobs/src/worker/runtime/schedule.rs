//! 每周拍卖轮次与赛季早鸟场次 bootstrap，以及 MoogleMail 暂停/恢复对账。

use auction_application::admin::{
    AdminMailPollStateRequest, AdminMailPollingCommand, AdminMailTransactionError,
};
use auction_application::auction::AuctionPolicyTemplate;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, SafeErrorKind,
};
use auction_application::core::{ApplicationError, Clock, RequestHash};
use auction_application::scheduling::{ScheduledRoundBootstrap, ScheduledRoundSnapshot};
use auction_domain::{
    AuctionEarlyBirdCalendar, AuctionId, AuctionScheduleSlot, AuctionScheduleSource,
    AuctionWeeklySchedule, CommandId, legacy_natural_key_uuid,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;

use super::{PersistentWorker, WorkerResult};
use crate::worker::diagnostics;

const MAIL_PAUSE_LEAD: Duration = Duration::hours(2);

/// 周计划的读取、幂等 bootstrap 与邮箱对账共用一次预算，不阻塞后续生命周期与结算阶段。
pub(crate) async fn run_schedule_bootstrap<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let deadline = tokio::time::Instant::now() + worker.auction.item_timeout;
    match tokio::time::timeout_at(deadline, run_schedule_bootstrap_inner(worker, deadline)).await {
        Ok(result) if tokio::time::Instant::now() < deadline => result,
        diagnostic_outcome @ (Ok(_) | Err(..)) => {
            diagnostics::unpersisted_error(
                "auction_schedule",
                if diagnostic_outcome.is_err() { "bootstrap_timeout" } else { "bootstrap_expired" },
                if diagnostic_outcome.is_err() {
                    SafeErrorKind::TIMEOUT
                } else {
                    SafeErrorKind::LEASE_EXPIRED
                },
                DiagnosticContext::default(),
            );
            drop(diagnostic_outcome);

            // 提交可能已发生；下轮读取冻结轮次和邮箱版本恢复，不补造失败事实或新命令身份。

            WorkerResult::Error
        }
    }
}

async fn run_schedule_bootstrap_inner<C>(
    worker: &PersistentWorker<C>,
    deadline: tokio::time::Instant,
) -> WorkerResult
where
    C: Clock + 'static,
{
    if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
        return WorkerResult::Success;
    }
    let Some(config) = worker.schedule_runtime.as_ref() else {
        return WorkerResult::Success;
    };
    if !config.validate() {
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!("auction weekly schedule configuration is invalid");
        return WorkerResult::Error;
    }
    let Ok(now) = worker.schedule.database_now().await else {
        tracing::warn!("读取周计划数据库时间失败");
        return WorkerResult::Error;
    };
    if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
        return WorkerResult::Success;
    }
    let Ok(generation) = generation(config, now) else {
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!("生成当前本地周拍卖计划失败");
        return WorkerResult::Error;
    };
    let Some(world_reader) = worker.worlds.as_deref() else {
        diagnostics::unpersisted(SafeErrorKind::CONFIGURATION_INVALID);
        tracing::error!("周计划缺少数据库业务世界读取端口");
        return WorkerResult::Error;
    };
    if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
        return WorkerResult::Success;
    }
    let worlds = match world_reader.list_worlds().await {
        Ok(worlds) => worlds,
        Err(error) => {
            diagnostics::unpersisted_error(
                "auction_schedule",
                "read_worlds",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            return WorkerResult::Error;
        }
    };

    let mut result = WorkerResult::Success;
    for world_id in &config.worlds {
        if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(world) = worlds.iter().find(|world| world.id.as_str() == world_id.as_str()) else {
            tracing::error!(world_id, "周计划配置的业务世界不存在");
            result = WorkerResult::Error;
            continue;
        };
        if !world.validate() {
            tracing::error!(world_id, "周计划读取到无效的业务世界投影");
            result = WorkerResult::Error;
            continue;
        }
        if !world.enabled {
            tracing::debug!(world_id, "业务世界已关闭，跳过当前周拍卖 bootstrap");
            continue;
        }
        let early_bird =
            config.early_bird.as_ref().filter(|calendar| calendar.applies_to(world_id));
        // 本周常规时段被早鸟覆盖期替代时改维护当前早鸟场次；全部场次结束后本周不再建轮次。
        let target = match early_bird {
            Some(calendar) if calendar.suppresses(world_id, &generation.times) => {
                match calendar.current_session(world_id, &generation.times, now) {
                    Ok(slot) => slot,
                    Err(error) => {
                        diagnostics::error(
                            "auction_schedule",
                            "resolve_calendar",
                            error.diagnostic_kind(),
                            DiagnosticContext {
                                disposition: Some(DiagnosticDisposition::Unpersisted),
                                ..Default::default()
                            },
                        );
                        result = WorkerResult::Error;
                        continue;
                    }
                }
            }
            _ => Some(generation.clone()),
        };
        let snapshot = match target.as_ref() {
            Some(slot) => {
                match load_or_bootstrap_round(worker, config, world_id, slot, now, deadline).await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "auction_schedule",
                            "load_round",
                            error.diagnostic_kind(),
                            DiagnosticContext::default(),
                        );
                        result = WorkerResult::Error;
                        continue;
                    }
                }
            }
            None => None,
        };
        if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
            break;
        }
        let gate = match early_bird {
            Some(calendar) => {
                match early_bird_intake_gate(
                    worker,
                    calendar,
                    world_id,
                    target.as_ref(),
                    snapshot.as_ref(),
                    now,
                )
                .await
                {
                    Ok(gate) => gate,
                    Err(error) => {
                        diagnostics::unpersisted_error(
                            "auction_schedule",
                            "reconcile_mail_gate",
                            error.diagnostic_kind(),
                            DiagnosticContext::default(),
                        );
                        result = WorkerResult::Error;
                        continue;
                    }
                }
            }
            None => None,
        };
        let desired = match (gate, snapshot.as_ref()) {
            (Some(gate), _) => Some(gate),
            (None, Some(snapshot)) => round_mail_gate(snapshot, now),
            (None, None) => None,
        };
        let Some(desired) = desired else {
            continue;
        };
        if let Err(error) =
            reconcile_mail(config, &worker.control, &desired, world_id, deadline).await
        {
            diagnostics::unpersisted_error(
                "auction_schedule",
                "reconcile_mail",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            result = WorkerResult::Error;
        }
    }
    result
}

/// 一次邮箱暂停/恢复期望；身份片段用于派生稳定命令 ID。
#[derive(Debug, Clone, PartialEq, Eq)]
struct MailGate {
    paused: bool,
    round_identity: String,
}

/// 赛季会清空异世界资产：首场早鸟草稿建立后至赛季开始前暂停收件，赛季开始后、首场预览前恢复。
/// 安排赛季前场次（序号 0）时，首场草稿在它开拍时才作为后继建立，旧赛季投稿仍可进入赛季前场次。
/// 该门禁优先于前一轮次的恢复期望；其余时段沿用轮次自身的开拍前暂停规则。
async fn early_bird_intake_gate<C>(
    worker: &PersistentWorker<C>,
    calendar: &AuctionEarlyBirdCalendar,
    world_id: &str,
    target: Option<&AuctionScheduleSlot>,
    snapshot: Option<&ScheduledRoundSnapshot>,
    now: DateTime<Utc>,
) -> Result<Option<MailGate>, ApplicationError>
where
    C: Clock + 'static,
{
    if !calendar.pauses_intake_before_season() {
        return Ok(None);
    }
    // 赛季已开始且当前维护的是首场草稿：恢复收件，让玩家在首场预览前投稿新赛季装备。
    // 已有轮次以冻结预览时间及完成事实为准，不能用重算模板恢复已进入开拍前暂停的邮箱。
    if let Some(slot) = target
        && let Some(key) = slot.source.early_bird
        && key.session == 1
        && snapshot.is_none_or(|round| round.preview_completed_at.is_none())
        && now < snapshot.map_or(slot.times.preview_due_at, |round| round.preview_due_at)
        && now >= calendar.season_of(key)?.window_start
    {
        return Ok(Some(MailGate { paused: false, round_identity: key.key() }));
    }
    // 下一赛季的首场草稿已由后继或 bootstrap 建立、赛季尚未开始：暂停收件。
    let Some(season) = calendar.upcoming_season(world_id, now) else {
        return Ok(None);
    };
    let Some(slot_key) = calendar.session(season.season, 1)?.slot_key() else {
        return Ok(None);
    };
    let round = worker.schedule.find_slot_round(world_id.to_owned(), slot_key.clone()).await?;
    Ok(round
        .filter(|round| round.active && round.preview_completed_at.is_none())
        .map(|_| MailGate { paused: true, round_identity: slot_key }))
}

/// 轮次自身的邮箱期望：Preview 后开拍前两小时暂停，开拍或取消/错过后恢复。
fn round_mail_gate(snapshot: &ScheduledRoundSnapshot, now: DateTime<Utc>) -> Option<MailGate> {
    let paused = if snapshot.active
        && snapshot.preview_completed_at.is_some()
        && snapshot.open_completed_at.is_none()
        && now >= snapshot.open_due_at - MAIL_PAUSE_LEAD
    {
        true
    } else if (!snapshot.active && snapshot.preview_completed_at.is_some())
        || (snapshot.active && snapshot.open_completed_at.is_some() && now >= snapshot.open_due_at)
    {
        // 取消/错过的轮次已经不再要求暂停；不能把本周邮箱永久留在 Preview 的暂停状态。
        false
    } else {
        return None;
    };
    Some(MailGate { paused, round_identity: round_identity(snapshot) })
}

fn round_identity(snapshot: &ScheduledRoundSnapshot) -> String {
    snapshot.slot_key.clone().unwrap_or_else(|| snapshot.local_week.format("%Y-%m-%d").to_string())
}

async fn load_or_bootstrap_round<C>(
    worker: &PersistentWorker<C>,
    config: &super::AuctionScheduleRuntimeConfig,
    world_id: &str,
    generation: &AuctionScheduleSlot,
    now: DateTime<Utc>,
    deadline: tokio::time::Instant,
) -> Result<Option<ScheduledRoundSnapshot>, ApplicationError>
where
    C: Clock + 'static,
{
    // 先读冻结事实，避免新经济模板或排期被当成旧 command_id 的另一份创建负载。
    // 即使新配置的预览窗口尚未到达，也必须按既有轮次的时间继续对账邮箱暂停/恢复。
    let slot_key = generation.slot_key();
    let existing = match &slot_key {
        Some(slot_key) => {
            worker.schedule.find_slot_round(world_id.to_owned(), slot_key.clone()).await?
        }
        None => {
            worker.schedule.find_current_round(world_id.to_owned(), generation.local_week).await?
        }
    };
    // 冻结事实回执不能越过 drain 或原截止后再授权创建；timeout 可能先 poll 出迟到回执。
    if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
        return Ok(None);
    }
    let snapshot = if let Some(snapshot) = existing {
        snapshot
    } else {
        let times = &generation.times;
        if now < times.preview_due_at && now < times.open_due_at - MAIL_PAUSE_LEAD {
            return Ok(None);
        }
        let identity = identities(world_id, &generation.identity_key());
        // 与开拍后继同一规则：赛季前场次冻结 v2 恢复策略，窗口内场次冻结赛季开始为收件起点。
        let early_bird_slot = generation.source.early_bird;
        let policy = if early_bird_slot.is_some_and(|slot| slot.is_pre_season()) {
            config.policy.for_pre_season()
        } else {
            config.policy.clone()
        };
        let intake_opens_at = match (early_bird_slot, config.early_bird.as_ref()) {
            (Some(slot), Some(calendar)) => calendar.intake_opens_at(slot)?,
            _ => None,
        };
        let request = BootstrapRequest {
            round_id: identity.round_id,
            auction_id: identity.auction_id,
            create_command_id: identity.create_command_id,
            world_id: world_id.to_owned(),
            local_week: generation.local_week,
            slot_key: slot_key.clone(),
            preview_due_at: times.preview_due_at,
            open_due_at: times.open_due_at,
            scheduled_end_at: times.scheduled_end_at,
            policy,
            schedule_source: generation.source.clone(),
            intake_opens_at,
        }
        .into_bootstrap()?;
        if worker.control.is_draining() || tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        // 只在尚无轮次时发起创建；并发创建仍由存储层的锁、摘要和既有一致性检查保护。
        worker.schedule.ensure_current_round(request).await?
    };
    if snapshot.world_id != world_id
        || snapshot.local_week != generation.local_week
        || snapshot.slot_key != slot_key
        // 已登记的 Admin 首期/后继拥有自己的稳定 ID，不能用自动 bootstrap 的 ID 反向拒绝。
        || snapshot.round_id.is_nil()
        || snapshot.create_command_id.is_nil()
        || snapshot.active != snapshot.auction_id.is_some()
        || snapshot.auction_id.is_some_and(|auction_id| auction_id.is_nil())
        || snapshot.open_completed_at.is_some() && snapshot.preview_completed_at.is_none()
        || snapshot.preview_due_at >= snapshot.open_due_at
        || snapshot.open_due_at >= snapshot.scheduled_end_at
    {
        return Err(ApplicationError::CapabilityUnavailable("周计划冻结投影无效"));
    }
    Ok(Some(snapshot))
}

#[derive(Debug, Clone)]
struct BootstrapRequest {
    round_id: uuid::Uuid,
    auction_id: AuctionId,
    create_command_id: CommandId,
    world_id: String,
    local_week: NaiveDate,
    slot_key: Option<String>,
    preview_due_at: DateTime<Utc>,
    open_due_at: DateTime<Utc>,
    scheduled_end_at: DateTime<Utc>,
    policy: AuctionPolicyTemplate,
    schedule_source: AuctionScheduleSource,
    intake_opens_at: Option<DateTime<Utc>>,
}

impl BootstrapRequest {
    fn into_bootstrap(
        self,
    ) -> Result<ScheduledRoundBootstrap, auction_application::core::ApplicationError> {
        let Self {
            round_id,
            auction_id,
            create_command_id,
            world_id,
            local_week,
            slot_key,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
            policy,
            schedule_source,
            intake_opens_at,
        } = self;
        #[derive(Serialize)]
        struct IdentityRequest<'a> {
            world_id: &'a str,
            local_week: NaiveDate,
            preview_due_at: DateTime<Utc>,
            open_due_at: DateTime<Utc>,
            scheduled_end_at: DateTime<Utc>,
            policy: serde_json::Value,
            schedule_source: &'a AuctionScheduleSource,
        }
        let request_hash = RequestHash::from_request(
            "scheduled_round_create",
            round_id,
            &IdentityRequest {
                world_id: &world_id,
                local_week,
                preview_due_at,
                open_due_at,
                scheduled_end_at,
                policy: policy.snapshot(scheduled_end_at),
                schedule_source: &schedule_source,
            },
        )?;
        Ok(ScheduledRoundBootstrap {
            round_id,
            auction_id,
            create_command_id,
            world_id,
            local_week,
            slot_key,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
            request_hash,
            policy,
            schedule_source,
            intake_opens_at,
        })
    }
}

async fn reconcile_mail(
    config: &super::AuctionScheduleRuntimeConfig,
    control: &crate::WorkerControlHandle,
    gate: &MailGate,
    world_id: &str,
    deadline: tokio::time::Instant,
) -> Result<(), auction_application::core::ApplicationError> {
    let Some(mail) = config.mail.as_ref() else {
        return Ok(());
    };
    let Some(mailbox_key) = config
        .mailboxes
        .iter()
        .find(|(configured_world, _)| configured_world == world_id)
        .map(|(_, mailbox_key)| mailbox_key.clone())
    else {
        return Ok(());
    };
    let desired = gate.paused;
    let identity = identities(world_id, &gate.round_identity);
    let base_command_id =
        if desired { identity.pause_command_id } else { identity.resume_command_id };
    #[derive(Serialize)]
    struct PauseRequest<'a> {
        mailbox_key: &'a str,
        paused: bool,
        expected_state_version: i64,
    }
    for _ in 0..3 {
        if control.is_draining() || tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        let state =
            mail.poll_state(AdminMailPollStateRequest { mailbox_key: mailbox_key.clone() }).await?;
        // 状态读取与版本冲突重试均可能跨过 drain/截止；已提交命令不回滚，未开始的写入停止。
        if control.is_draining()
            || tokio::time::Instant::now() >= deadline
            || state.paused == desired
        {
            return Ok(());
        }
        // 同一观测版本保持稳定；Admin 改动后的新版本必须使用新命令，不能重放本周旧结果。
        let version = state.state_version.to_be_bytes();
        let command_id = CommandId::from_uuid(legacy_natural_key_uuid(
            "auction-schedule-mail-poll-state",
            &[base_command_id.as_uuid().as_bytes(), mailbox_key.as_bytes(), &version],
        ));
        let request_hash = RequestHash::from_request(
            "set_polling_paused",
            command_id.as_uuid(),
            &PauseRequest {
                mailbox_key: &mailbox_key,
                paused: desired,
                expected_state_version: state.state_version,
            },
        )?;
        if control.is_draining() || tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        match mail
            .set_polling_paused(AdminMailPollingCommand {
                command_id,
                mailbox_key: mailbox_key.clone(),
                paused: desired,
                expected_state_version: state.state_version,
                actor_id: "auction-worker".to_owned(),
                request_id: None,
                request_hash,
            })
            .await
        {
            Ok(_) => return Ok(()),
            Err(ApplicationError::AdminMailTransaction(
                AdminMailTransactionError::StateConflict { .. },
            )) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(auction_application::core::ApplicationError::CapabilityUnavailable(
        "MoogleMail 轮询状态版本持续变化",
    ))
}

/// 当前本地周的常规周时段；与早鸟日历、开拍后继共用同一周计划投影。
fn generation(
    config: &super::AuctionScheduleRuntimeConfig,
    now: DateTime<Utc>,
) -> Result<AuctionScheduleSlot, ()> {
    let weekly = AuctionWeeklySchedule::new(
        &config.timezone,
        (config.preview_weekday, config.preview_minute),
        (config.open_weekday, config.open_minute),
        (config.end_weekday, config.end_minute),
    )
    .map_err(|error| {
        diagnostics::warn(
            "auction_schedule",
            "validate_weekly_schedule",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );
    })?;
    weekly.slot(weekly.week_start(now)).map_err(|error| {
        diagnostics::warn(
            "auction_schedule",
            "resolve_weekly_slot",
            error.diagnostic_kind(),
            DiagnosticContext::default(),
        );
    })
}

struct Identities {
    round_id: uuid::Uuid,
    auction_id: AuctionId,
    create_command_id: CommandId,
    pause_command_id: CommandId,
    resume_command_id: CommandId,
}

/// 常规周以 `YYYY-MM-DD` 为身份片段，早鸟场次以时段键为身份片段；与开拍后继的派生算法一致。
fn identities(world_id: &str, round_identity: &str) -> Identities {
    let id = |kind: &str| {
        legacy_natural_key_uuid(
            "auction-schedule",
            &[kind.as_bytes(), world_id.as_bytes(), round_identity.as_bytes()],
        )
    };
    Identities {
        round_id: id("round"),
        auction_id: AuctionId::from_uuid(id("auction")),
        create_command_id: CommandId::from_uuid(id("create-command")),
        pause_command_id: CommandId::from_uuid(id("pause-polling")),
        resume_command_id: CommandId::from_uuid(id("resume-polling")),
    }
}
