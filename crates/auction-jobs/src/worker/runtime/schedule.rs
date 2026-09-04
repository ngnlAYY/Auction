//! 每周拍卖轮次 bootstrap 与 MoogleMail 暂停/恢复对账。

use auction_application::{
    AdminMailPollStateRequest, AdminMailPollingCommand, AdminMailTransactionError,
    ApplicationError, AuctionPolicyTemplate, Clock, RequestHash, ScheduledRoundBootstrap,
    ScheduledRoundSnapshot,
};
use auction_domain::{AuctionId, CommandId, legacy_natural_key_uuid};
use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc,
};
use chrono_tz::Tz;
use serde::Serialize;

use super::{PersistentWorker, WorkerResult};

const MAIL_PAUSE_LEAD: Duration = Duration::hours(2);

pub(crate) async fn run_schedule_bootstrap<C>(worker: &PersistentWorker<C>) -> WorkerResult
where
    C: Clock + 'static,
{
    let Some(config) = worker.schedule_runtime.as_ref() else {
        return WorkerResult::Success;
    };
    if !config.validate() {
        tracing::error!("auction weekly schedule configuration is invalid");
        return WorkerResult::Error;
    }
    let Ok(now) = worker.schedule.database_now().await else {
        tracing::warn!("读取周计划数据库时间失败");
        return WorkerResult::Error;
    };
    let Ok(generation) = generation(config, now) else {
        tracing::error!("生成当前本地周拍卖计划失败");
        return WorkerResult::Error;
    };
    if now < generation.preview_due_at && now < generation.open_due_at - MAIL_PAUSE_LEAD {
        return WorkerResult::Success;
    }

    let Some(world_reader) = worker.worlds.as_deref() else {
        tracing::error!("周计划缺少数据库业务世界读取端口");
        return WorkerResult::Error;
    };
    let worlds = match world_reader.list_worlds().await {
        Ok(worlds) => worlds,
        Err(error) => {
            tracing::warn!(error = %error, "读取周计划业务世界状态失败");
            return WorkerResult::Error;
        }
    };

    let mut result = WorkerResult::Success;
    for world_id in &config.worlds {
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
        let identity = identities(world_id, generation.local_week);
        let request = BootstrapRequest {
            round_id: identity.round_id,
            auction_id: identity.auction_id,
            create_command_id: identity.create_command_id,
            world_id: world_id.clone(),
            local_week: generation.local_week,
            preview_due_at: generation.preview_due_at,
            open_due_at: generation.open_due_at,
            scheduled_end_at: generation.scheduled_end_at,
            policy: config.policy.clone(),
        };
        let request = match request.into_bootstrap() {
            Ok(request) => request,
            Err(error) => {
                tracing::error!(world_id, error = %error, "当前周拍卖 bootstrap 命令摘要生成失败");
                result = WorkerResult::Error;
                continue;
            }
        };
        let snapshot = match worker.schedule.ensure_current_round(request).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(world_id, error = %error, "当前周拍卖轮次 bootstrap 失败");
                result = WorkerResult::Error;
                continue;
            }
        };
        if let Err(error) = reconcile_mail(
            config,
            &snapshot,
            world_id,
            generation.local_week,
            generation.open_due_at,
            now,
        )
        .await
        {
            tracing::warn!(world_id, error = %error, "MoogleMail 周计划暂停/恢复对账失败");
            result = WorkerResult::Error;
        }
    }
    result
}

#[derive(Debug, Clone)]
struct BootstrapRequest {
    round_id: uuid::Uuid,
    auction_id: AuctionId,
    create_command_id: CommandId,
    world_id: String,
    local_week: NaiveDate,
    preview_due_at: DateTime<Utc>,
    open_due_at: DateTime<Utc>,
    scheduled_end_at: DateTime<Utc>,
    policy: AuctionPolicyTemplate,
}

impl BootstrapRequest {
    fn into_bootstrap(
        self,
    ) -> Result<ScheduledRoundBootstrap, auction_application::ApplicationError> {
        let Self {
            round_id,
            auction_id,
            create_command_id,
            world_id,
            local_week,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
            policy,
        } = self;
        #[derive(Serialize)]
        struct IdentityRequest<'a> {
            world_id: &'a str,
            local_week: NaiveDate,
            preview_due_at: DateTime<Utc>,
            open_due_at: DateTime<Utc>,
            scheduled_end_at: DateTime<Utc>,
            policy: serde_json::Value,
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
            },
        )?;
        Ok(ScheduledRoundBootstrap {
            round_id,
            auction_id,
            create_command_id,
            world_id,
            local_week,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
            request_hash,
            policy,
        })
    }
}

async fn reconcile_mail(
    config: &super::AuctionScheduleRuntimeConfig,
    snapshot: &ScheduledRoundSnapshot,
    world_id: &str,
    local_week: NaiveDate,
    open_due_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), auction_application::ApplicationError> {
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
    let desired = if snapshot.active
        && snapshot.preview_completed_at.is_some()
        && snapshot.open_completed_at.is_none()
        && now >= open_due_at - MAIL_PAUSE_LEAD
    {
        Some(true)
    } else if snapshot.active && snapshot.open_completed_at.is_some() && now >= open_due_at {
        Some(false)
    } else {
        None
    };
    let Some(desired) = desired else { return Ok(()) };
    let identity = identities(world_id, local_week);
    let command_id = if desired { identity.pause_command_id } else { identity.resume_command_id };
    #[derive(Serialize)]
    struct PauseRequest<'a> {
        mailbox_key: &'a str,
        paused: bool,
    }
    let request_hash = RequestHash::from_request(
        "set_polling_paused",
        command_id.as_uuid(),
        &PauseRequest { mailbox_key: &mailbox_key, paused: desired },
    )?;
    for _ in 0..3 {
        let state =
            mail.poll_state(AdminMailPollStateRequest { mailbox_key: mailbox_key.clone() }).await?;
        if state.paused == desired {
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
    Err(auction_application::ApplicationError::CapabilityUnavailable(
        "MoogleMail 轮询状态版本持续变化",
    ))
}

fn generation(
    config: &super::AuctionScheduleRuntimeConfig,
    now: DateTime<Utc>,
) -> Result<Generation, ()> {
    let timezone: Tz = config.timezone.parse().map_err(|_| ())?;
    let local = now.with_timezone(&timezone);
    let week_start =
        local.date_naive() - Duration::days(i64::from(local.weekday().num_days_from_monday()));
    Ok(Generation {
        local_week: week_start,
        preview_due_at: at(&timezone, week_start, config.preview_weekday, config.preview_minute)?,
        open_due_at: at(&timezone, week_start, config.open_weekday, config.open_minute)?,
        scheduled_end_at: at(&timezone, week_start, config.end_weekday, config.end_minute)?,
    })
}

fn at(timezone: &Tz, week_start: NaiveDate, weekday: u8, minute: u16) -> Result<DateTime<Utc>, ()> {
    let date = week_start + Duration::days(i64::from(weekday));
    let time = NaiveTime::from_num_seconds_from_midnight_opt(u32::from(minute) * 60, 0).ok_or(())?;
    let local = NaiveDateTime::new(date, time);
    if let Some(value) = timezone.from_local_datetime(&local).earliest() {
        return Ok(value.with_timezone(&Utc));
    }

    // Go `time.Date` 对春季跳时使用跳时后的 offset 归一化不存在的墙上时间；沿用该行为，
    // 避免旧版可生成的周计划在 IANA DST 边界被新版直接拒绝。
    let offset = (1_i64..=48 * 60).find_map(|minutes| {
        local
            .checked_add_signed(Duration::minutes(minutes))
            .and_then(|candidate| timezone.offset_from_local_datetime(&candidate).earliest())
    });
    let offset_seconds = i64::from(offset.ok_or(())?.fix().local_minus_utc());
    let utc = local.checked_sub_signed(Duration::seconds(offset_seconds)).ok_or(())?;
    Ok(DateTime::from_naive_utc_and_offset(utc, Utc))
}

struct Generation {
    local_week: NaiveDate,
    preview_due_at: DateTime<Utc>,
    open_due_at: DateTime<Utc>,
    scheduled_end_at: DateTime<Utc>,
}

struct Identities {
    round_id: uuid::Uuid,
    auction_id: AuctionId,
    create_command_id: CommandId,
    pause_command_id: CommandId,
    resume_command_id: CommandId,
}

fn identities(world_id: &str, local_week: NaiveDate) -> Identities {
    let week = local_week.format("%Y-%m-%d").to_string();
    let id = |kind: &str| {
        legacy_natural_key_uuid(
            "auction-schedule",
            &[kind.as_bytes(), world_id.as_bytes(), week.as_bytes()],
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
