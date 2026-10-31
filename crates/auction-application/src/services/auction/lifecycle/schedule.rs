use super::*;

use crate::{AuctionScheduleSlot, AuctionScheduleSource, AuctionScheduleTimes};

/// 冻结周链遇到早鸟替代时向后顺延的最大周数，覆盖约两年。
const MAX_SUPPRESSED_WEEKS: i64 = 106;

/// 开拍或取消 Preview 时计算下一期草稿的时段与稳定身份。
/// 常规周后继沿用冻结来源的七天规则；启用早鸟期时，被早鸟覆盖期替代的周时段继续顺延，
/// 早鸟场次更早时改用早鸟场次，早鸟场次之后按启动期有效的周计划恢复。
pub(super) fn successor_plan(
    context: &AuctionLifecycleContext,
    policy: &AuctionPolicyTemplate,
    calendar: Option<&AuctionScheduleCalendar>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<AuctionPreviewSuccessorPlan, ApplicationError> {
    // 后继是新聚合：使用当前有效模板，旧聚合与已接受命令仍按冻结事实重放。
    policy.validate()?;
    let next = next_slot(context, calendar, now)?;
    let NextSlot { local_week, source: schedule_source, times } = next;
    let early_bird_slot = schedule_source.as_ref().and_then(|source| source.early_bird);
    // 赛季前场次冻结 24 小时付款期限的 v2 恢复策略；其余场次沿用模板。
    let pre_season_policy;
    let policy = if early_bird_slot.is_some_and(|slot| slot.is_pre_season()) {
        pre_season_policy = policy.for_pre_season();
        &pre_season_policy
    } else {
        policy
    };
    // 窗口内早鸟场次在赛季开始前拒收装备；日历缺失时来源不会带早鸟身份。
    let intake_opens_at = match (
        early_bird_slot,
        calendar.and_then(|calendar| calendar.early_bird_for(&context.world_id)),
    ) {
        (Some(slot), Some(early_bird)) => early_bird.intake_opens_at(slot)?,
        _ => None,
    };
    let sequence =
        context.sequence.checked_add(1).ok_or_else(|| lifecycle_state("下一期拍卖序号溢出"))?;
    let local_week_text = local_week.format("%Y-%m-%d").to_string();
    // 常规周继续以 world/local_week 派生身份；早鸟场次以时段键派生，允许同周多场。
    let identity = early_bird_slot.map_or_else(|| local_week_text.clone(), |slot| slot.key());
    let round_id = schedule_id("round", &context.world_id, &identity);
    let auction_id = AuctionId::from_uuid(schedule_id("auction", &context.world_id, &identity));
    let create_command_id =
        CommandId::from_uuid(schedule_id("create-command", &context.world_id, &identity));
    let title = format!("{} #{}", context.world_id, sequence);

    #[derive(Serialize)]
    struct ScheduledCreateRequest<'a> {
        world_id: &'a str,
        sequence: i64,
        local_week: &'a str,
        preview_due_at: chrono::DateTime<chrono::Utc>,
        open_due_at: chrono::DateTime<chrono::Utc>,
        scheduled_end_at: chrono::DateTime<chrono::Utc>,
        policy: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedule_source: &'a Option<crate::AuctionScheduleSource>,
    }
    let create_request_hash = RequestHash::from_request(
        "scheduled_auction_create",
        auction_id.as_uuid(),
        &ScheduledCreateRequest {
            world_id: &context.world_id,
            sequence,
            local_week: &local_week_text,
            preview_due_at: times.preview_due_at,
            open_due_at: times.open_due_at,
            scheduled_end_at: times.scheduled_end_at,
            policy: policy.snapshot(times.scheduled_end_at),
            schedule_source: &schedule_source,
        },
    )?;

    Ok(AuctionPreviewSuccessorPlan {
        policy: policy.clone(),
        schedule_source,
        round_id,
        auction_id,
        create_command_id,
        create_request_hash,
        local_week,
        sequence,
        title,
        preview_due_at: times.preview_due_at,
        open_due_at: times.open_due_at,
        scheduled_end_at: times.scheduled_end_at,
        intake_opens_at,
    })
}

/// 后继时段；没有来源的历史轮次只保留 UTC 时间。
struct NextSlot {
    local_week: NaiveDate,
    source: Option<AuctionScheduleSource>,
    times: AuctionScheduleTimes,
}

impl From<AuctionScheduleSlot> for NextSlot {
    fn from(slot: AuctionScheduleSlot) -> Self {
        Self { local_week: slot.local_week, source: Some(slot.source), times: slot.times }
    }
}

fn next_slot(
    context: &AuctionLifecycleContext,
    calendar: Option<&AuctionScheduleCalendar>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<NextSlot, ApplicationError> {
    let origin_early_bird =
        context.schedule_source.as_ref().is_some_and(|source| source.early_bird.is_some());
    let Some(calendar) = calendar else {
        // 周排期未启用时没有日历：早鸟场次按原本地钟点顺延一周并退回周身份，其余保持原规则。
        return if origin_early_bird {
            early_bird_fallback(context)
        } else {
            frozen_weekly(context, 1)
        };
    };
    let early_bird = calendar.early_bird_for(&context.world_id);
    let suppressed = |times: &AuctionScheduleTimes| {
        early_bird.is_some_and(|calendar| calendar.suppresses(&context.world_id, times))
    };
    // 下一期预览不早于本场计划结束，避免计划内两场同时竞价；开拍迟到时以实际时刻为准。
    let floor = context.policy_scheduled_end_at.max(now);
    let weekly = if origin_early_bird {
        // 早鸟链没有冻结的周来源，按启动期有效周计划恢复，与 bootstrap 的创建规则一致。
        calendar.weekly().first_slot_from(floor, suppressed)?.into()
    } else {
        let mut weeks = 1;
        loop {
            let candidate = frozen_weekly(context, weeks)?;
            if !suppressed(&candidate.times) {
                break candidate;
            }
            weeks += 1;
            if weeks > MAX_SUPPRESSED_WEEKS {
                return Err(lifecycle_state("早鸟期替代后找不到下一周时段"));
            }
        }
    };
    let Some(early_bird) = early_bird else {
        return Ok(weekly);
    };
    match early_bird.first_session_from(&context.world_id, floor)? {
        Some(session) if session.times.preview_due_at < weekly.times.preview_due_at => {
            Ok(session.into())
        }
        _ => Ok(weekly),
    }
}

/// 冻结周链向后推进 `weeks` 周；历史轮次未保存时区来源时保留原 UTC 间隔，
/// 不能用当前配置重解释已冻结计划。
fn frozen_weekly(
    context: &AuctionLifecycleContext,
    weeks: i64,
) -> Result<NextSlot, ApplicationError> {
    let local_week = checked_add_days(current_week(context), 7 * weeks, "下一期周日期超出范围")?;
    if let Some(source) = &context.schedule_source {
        let mut next = source.clone();
        for _ in 0..weeks {
            next = next.next_week()?;
        }
        let times = next.resolve()?;
        return Ok(NextSlot { local_week, source: Some(next), times });
    }
    let shift = |value: chrono::DateTime<chrono::Utc>, message| {
        value.checked_add_signed(Duration::days(7 * weeks)).ok_or_else(|| lifecycle_state(message))
    };
    Ok(NextSlot {
        local_week,
        source: None,
        times: AuctionScheduleTimes {
            preview_due_at: shift(context.preview_scheduled_at, "下一期预览时间超出范围")?,
            open_due_at: shift(context.scheduled_open_at, "下一期开拍时间超出范围")?,
            scheduled_end_at: shift(context.policy_scheduled_end_at, "下一期结束时间超出范围")?,
        },
    })
}

fn early_bird_fallback(context: &AuctionLifecycleContext) -> Result<NextSlot, ApplicationError> {
    let local_week = checked_add_days(current_week(context), 7, "下一期周日期超出范围")?;
    let source = context
        .schedule_source
        .as_ref()
        .ok_or_else(|| lifecycle_state("早鸟场次缺少排期来源"))?
        .shifted_local_days(7)?;
    let times = source.resolve()?;
    Ok(NextSlot { local_week, source: Some(source), times })
}

fn current_week(context: &AuctionLifecycleContext) -> NaiveDate {
    context.scheduled_round_local_week.unwrap_or_else(|| {
        context.preview_scheduled_at.date_naive()
            - Duration::days(i64::from(
                context.preview_scheduled_at.weekday().num_days_from_monday(),
            ))
    })
}

fn checked_add_days(
    value: NaiveDate,
    days: i64,
    message: &str,
) -> Result<NaiveDate, ApplicationError> {
    value.checked_add_signed(Duration::days(days)).ok_or_else(|| lifecycle_state(message))
}

/// 保持与旧版周计划相同的长度前缀 SHA-256 标识算法，避免迁移后同一 world/week 产生双轮次。
/// 早鸟场次以 `early-bird:YYYY-MM-DD:N` 作为同一位置的身份片段，与 Worker bootstrap 一致。
fn schedule_id(kind: &str, world_id: &str, identity: &str) -> Uuid {
    let mut digest = Sha256::new();
    for value in ["auction-schedule", kind, world_id, identity] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value.as_bytes());
    }
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
