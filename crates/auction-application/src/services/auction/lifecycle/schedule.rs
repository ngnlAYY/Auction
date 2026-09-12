use super::*;

pub(super) fn successor_plan(
    context: &AuctionLifecycleContext,
) -> Result<AuctionPreviewSuccessorPlan, ApplicationError> {
    context.policy_settlement.validate_for_new_auction()?;
    let current_week = context.scheduled_round_local_week.unwrap_or_else(|| {
        context.preview_scheduled_at.date_naive()
            - Duration::days(i64::from(
                context.preview_scheduled_at.weekday().num_days_from_monday(),
            ))
    });
    let local_week = checked_add_days(current_week, 7, "下一期周日期超出范围")?;
    let preview_due_at = checked_add_week(context.preview_scheduled_at, "下一期预览时间超出范围")?;
    let open_due_at = checked_add_week(context.scheduled_open_at, "下一期开拍时间超出范围")?;
    let scheduled_end_at =
        checked_add_week(context.policy_scheduled_end_at, "下一期结束时间超出范围")?;
    let sequence =
        context.sequence.checked_add(1).ok_or_else(|| lifecycle_state("下一期拍卖序号溢出"))?;
    let local_week_text = local_week.format("%Y-%m-%d").to_string();
    let round_id = schedule_id("round", &context.world_id, &local_week_text);
    let auction_id =
        AuctionId::from_uuid(schedule_id("auction", &context.world_id, &local_week_text));
    let create_command_id =
        CommandId::from_uuid(schedule_id("create-command", &context.world_id, &local_week_text));
    let title = format!("{} #{}", context.world_id, sequence);

    #[derive(Serialize)]
    struct ScheduledCreateRequest<'a> {
        world_id: &'a str,
        sequence: i64,
        local_week: &'a str,
        preview_due_at: chrono::DateTime<chrono::Utc>,
        open_due_at: chrono::DateTime<chrono::Utc>,
        scheduled_end_at: chrono::DateTime<chrono::Utc>,
    }
    let create_request_hash = RequestHash::from_request(
        "scheduled_auction_create",
        auction_id.as_uuid(),
        &ScheduledCreateRequest {
            world_id: &context.world_id,
            sequence,
            local_week: &local_week_text,
            preview_due_at,
            open_due_at,
            scheduled_end_at,
        },
    )?;

    Ok(AuctionPreviewSuccessorPlan {
        round_id,
        auction_id,
        create_command_id,
        create_request_hash,
        local_week,
        sequence,
        title,
        preview_due_at,
        open_due_at,
        scheduled_end_at,
    })
}

fn checked_add_week(
    value: chrono::DateTime<chrono::Utc>,
    message: &str,
) -> Result<chrono::DateTime<chrono::Utc>, ApplicationError> {
    value.checked_add_signed(Duration::days(7)).ok_or_else(|| lifecycle_state(message))
}

fn checked_add_days(
    value: NaiveDate,
    days: i64,
    message: &str,
) -> Result<NaiveDate, ApplicationError> {
    value.checked_add_signed(Duration::days(days)).ok_or_else(|| lifecycle_state(message))
}

/// 保持与旧版周计划相同的长度前缀 SHA-256 标识算法，避免迁移后同一 world/week 产生双轮次。
fn schedule_id(kind: &str, world_id: &str, local_week: &str) -> Uuid {
    let mut digest = Sha256::new();
    for value in ["auction-schedule", kind, world_id, local_week] {
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
