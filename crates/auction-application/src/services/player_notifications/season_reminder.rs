//! 换季前 isekai 领取与交付提醒：每个收件人每个冻结赛季边界一封。
//!
//! 边界只来自已冻结的 `auctions.intake_opens_at`；提醒正文与身份都绑定该边界，
//! 换季配置之后变化不会改写已创建通知。发送围栏前若边界已过或义务已完成即退役。

use std::collections::BTreeMap;

use auction_domain::CommandId;
use chrono::{DateTime, SecondsFormat, Utc};
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::RequestHash;
use crate::ports::mail::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
};
use crate::ports::player_notifications::{PlayerNotificationError, SeasonReminderFact};
use crate::services::public::{format_credits, lot_code};

use super::valid_text;

/// 提醒正文上限；超出时不截断义务列表，直接不创建通知。
pub const MAX_SEASON_REMINDER_BODY_BYTES: usize = 16 << 10;

/// 正文最多列出的未完成义务数；其余只给计数。
pub const MAX_SEASON_REMINDER_ITEMS: usize = 20;

/// 提醒的固定主题。
pub const SEASON_REMINDER_SUBJECT: &str = "Isekai season change: pending deliveries";

/// 单个未完成义务。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SeasonReminderItem {
    pub obligation_kind: String,
    /// lot 的全局唯一 ID；lot 编号只在单场拍卖内唯一。
    pub lot_id: Uuid,
    /// 场次内稳定序号；只用于排序/去重。
    pub lot_number: u32,
    /// 玩家显示的类别内编号。
    pub category_number: u32,
    pub category: String,
    pub equipment_name: String,
    pub amount: i64,
}

/// 一个收件人在一个冻结赛季边界前的未完成义务集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonReminder {
    pub season_boundary: DateTime<Utc>,
    pub recipient: String,
    pub items: Vec<SeasonReminderItem>,
}

/// 按 (冻结边界, 收件人) 合并扫描事实；结果按身份排序，重复扫描顺序稳定。
pub fn group_season_reminders(
    facts: Vec<SeasonReminderFact>,
) -> Result<Vec<SeasonReminder>, PlayerNotificationError> {
    let mut grouped: BTreeMap<(DateTime<Utc>, String), SeasonReminder> = BTreeMap::new();
    for fact in facts {
        if fact.season_boundary.timestamp_nanos_opt().is_none()
            || !valid_text(&fact.recipient, 256)
            || !valid_text(&fact.obligation_kind, 64)
            || fact.lot_number == 0
            || fact.category_number == 0
            || !valid_text(&fact.category, 128)
            || !valid_text(&fact.equipment_name, 512)
            || fact.amount < 0
        {
            return Err(PlayerNotificationError::InvalidSeasonReminder);
        }
        let key = (fact.season_boundary, fact.recipient.clone());
        let reminder = grouped.entry(key).or_insert_with(|| SeasonReminder {
            season_boundary: fact.season_boundary,
            recipient: fact.recipient.clone(),
            items: Vec::new(),
        });
        reminder.items.push(SeasonReminderItem {
            obligation_kind: fact.obligation_kind,
            lot_id: fact.lot_id,
            lot_number: fact.lot_number,
            category_number: fact.category_number,
            category: fact.category,
            equipment_name: fact.equipment_name,
            amount: fact.amount,
        });
    }
    let mut reminders = grouped.into_values().collect::<Vec<_>>();
    for reminder in &mut reminders {
        reminder.items.sort_by(|left, right| {
            left.lot_number
                .cmp(&right.lot_number)
                .then_with(|| left.obligation_kind.cmp(&right.obligation_kind))
                .then_with(|| left.lot_id.cmp(&right.lot_id))
        });
        // 同一 lot ID × 义务种类只能出现一次；lot 编号只在单场拍卖内唯一。
        if reminder.items.windows(2).any(|pair| {
            pair[0].lot_id == pair[1].lot_id && pair[0].obligation_kind == pair[1].obligation_kind
        }) {
            return Err(PlayerNotificationError::InvalidSeasonReminder);
        }
    }
    Ok(reminders)
}

/// 由已合并事实生成冻结正文；超出正文预算返回 `None`，不截断义务结果。
pub fn season_reminder_body(reminder: &SeasonReminder) -> Option<String> {
    let mut body = String::with_capacity(180 + reminder.items.len() * 96);
    body.push_str("The Isekai season change is approaching.\n\n");
    body.push_str(&format!(
        "Isekai closes at {}.\n\n",
        reminder.season_boundary.to_rfc3339_opts(SecondsFormat::Secs, true)
    ));
    body.push_str("You still have unfinished auction deliveries in Isekai:\n");
    for item in reminder.items.iter().take(MAX_SEASON_REMINDER_ITEMS) {
        let label = obligation_label(&item.obligation_kind);
        if item.amount > 0 {
            body.push_str(&format!(
                "- {}: {} - {} Credits ({})\n",
                lot_code(&item.category, item.category_number),
                item.equipment_name,
                format_credits(item.amount),
                label
            ));
        } else {
            body.push_str(&format!(
                "- {}: {} ({})\n",
                lot_code(&item.category, item.category_number),
                item.equipment_name,
                label
            ));
        }
    }
    let hidden = reminder.items.len().saturating_sub(MAX_SEASON_REMINDER_ITEMS);
    if hidden > 0 {
        body.push_str(&format!("- and {hidden} more\n"));
    }
    body.push_str("\nCollect the mails and finish these deliveries before the season ends.");
    (body.len() <= MAX_SEASON_REMINDER_BODY_BYTES).then_some(body)
}

/// 生成稳定入队命令；同一 (边界, 收件人) 重放得到同一 command_id 与请求摘要。
pub fn season_reminder_command(
    reminder: &SeasonReminder,
    body: String,
) -> Result<MailDeliveryCommand, PlayerNotificationError> {
    let boundary_epoch = reminder.season_boundary.timestamp();
    let notification_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/season-reminder-v1/{boundary_epoch}/{}", reminder.recipient).as_bytes(),
    );
    let origin_id = format!("season_reminder:{boundary_epoch}:{}", reminder.recipient);
    let request_hash = RequestHash::from_request(
        "season_reminder_notification",
        notification_id,
        &(boundary_epoch, &reminder.recipient, &reminder.items, &body),
    )
    .map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "season_reminder",
            "hash_notification",
            DiagnosticContext::default(),
        );
        PlayerNotificationError::Unavailable
    })?;
    Ok(MailDeliveryCommand {
        command_id: CommandId::from_uuid(notification_id),
        request_hash,
        origin: MailDeliveryOrigin::SeasonReminder,
        origin_id,
        operation: MailDeliveryOperation::Send,
        world_id: "isekai".to_owned(),
        // 冻结边界参与投递载荷，发送围栏与退役判定都读同一值。
        external_mail_id: format!(
            "season-end:{}",
            reminder.season_boundary.to_rfc3339_opts(SecondsFormat::Secs, true)
        ),
        recipient: reminder.recipient.clone(),
        template: Some(MailNotificationTemplate::IsekaiSeasonReminder),
        result_code: Some("season_end".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some(body),
    })
}

/// 发送围栏前的失效判定：边界已过或已无未完成义务时退役，不进入发送或人工对账。
pub fn season_reminder_invalid_reason(
    now: DateTime<Utc>,
    season_boundary: DateTime<Utc>,
    outstanding: usize,
) -> Option<&'static str> {
    if now >= season_boundary {
        Some("season_boundary_passed")
    } else if outstanding == 0 {
        Some("season_reminder_resolved")
    } else {
        None
    }
}

/// 义务种类的英文展示；未知种类使用中性文案，不泄漏内部状态名。
fn obligation_label(kind: &str) -> &'static str {
    match kind {
        "winner_payment_carrier_cod" => "payment carrier",
        "winner_equipment_handoff" => "equipment handoff",
        "seller_payout" => "seller payout",
        "buyer_refund" => "refund",
        "unsold_return" | "withdrawn_return" => "returned equipment",
        "winner_equipment_cod" => "cash on delivery",
        _ => "pending delivery",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn boundary() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).single().expect("合法时间")
    }

    fn fact(recipient: &str, lot_number: u32, kind: &str) -> SeasonReminderFact {
        fact_with_lot(recipient, lot_number, kind, Uuid::from_u128(u128::from(lot_number)))
    }

    fn fact_with_lot(
        recipient: &str,
        lot_number: u32,
        kind: &str,
        lot_id: Uuid,
    ) -> SeasonReminderFact {
        SeasonReminderFact {
            season_boundary: boundary(),
            recipient: recipient.to_owned(),
            obligation_kind: kind.to_owned(),
            lot_id,
            lot_number,
            category_number: lot_number,
            category: "Claw".to_owned(),
            equipment_name: format!("Item {lot_number}"),
            amount: 12_000,
        }
    }

    #[test]
    fn groups_outstanding_obligations_per_recipient_and_boundary() {
        let reminders = group_season_reminders(vec![
            fact("alice", 2, "winner_equipment_handoff"),
            fact("bob", 1, "seller_payout"),
            fact("alice", 1, "winner_payment_carrier_cod"),
        ])
        .expect("合法事实必须聚合");
        assert_eq!(reminders.len(), 2);
        let alice = reminders
            .iter()
            .find(|reminder| reminder.recipient == "alice")
            .expect("alice 提醒必须存在");
        assert_eq!(alice.items.len(), 2);
        assert_eq!(alice.items[0].obligation_kind, "winner_payment_carrier_cod");
    }

    #[test]
    fn duplicate_lot_numbers_from_different_auctions_are_distinct() {
        let reminders = group_season_reminders(vec![
            fact_with_lot("alice", 1, "winner_equipment_handoff", Uuid::from_u128(1)),
            fact_with_lot("alice", 1, "winner_equipment_handoff", Uuid::from_u128(2)),
        ])
        .expect("不同场次的同号 lot 必须都保留");
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].items.len(), 2);
        assert_ne!(reminders[0].items[0].lot_id, reminders[0].items[1].lot_id);
    }

    #[test]
    fn repeated_obligation_for_the_same_lot_is_rejected() {
        let duplicate = fact_with_lot("alice", 1, "winner_equipment_handoff", Uuid::from_u128(7));
        assert_eq!(
            group_season_reminders(vec![duplicate.clone(), duplicate]),
            Err(PlayerNotificationError::InvalidSeasonReminder)
        );
    }

    #[test]
    fn body_states_the_frozen_boundary_and_items() {
        let reminder = group_season_reminders(vec![fact("alice", 1, "winner_equipment_handoff")])
            .expect("合法事实必须聚合")
            .remove(0);
        let body = season_reminder_body(&reminder).expect("正文必须可生成");
        assert!(body.contains("2026-11-01T09:00:00Z"), "{body}");
        assert!(body.contains("Cla01") || body.contains("Lot01"), "{body}");
        assert!(body.contains("12,000"), "{body}");
        assert!(body.contains("equipment handoff"), "{body}");
    }

    #[test]
    fn command_freezes_the_season_boundary_identity() {
        let reminder = group_season_reminders(vec![fact("alice", 1, "seller_payout")])
            .expect("合法事实必须聚合")
            .remove(0);
        let body = season_reminder_body(&reminder).expect("正文必须可生成");
        let first = season_reminder_command(&reminder, body.clone()).expect("命令必须生成");
        let second = season_reminder_command(&reminder, body).expect("命令必须生成");
        assert_eq!(first.command_id, second.command_id);
        assert_eq!(first.request_hash, second.request_hash);
        assert_eq!(first.origin, MailDeliveryOrigin::SeasonReminder);
        assert_eq!(first.origin_id, "season_reminder:1793523600:alice");
        assert_eq!(first.external_mail_id, "season-end:2026-11-01T09:00:00Z");
        assert_eq!(first.world_id, "isekai");
    }

    #[test]
    fn reminder_retires_after_the_frozen_boundary_or_when_resolved() {
        let before = boundary() - chrono::Duration::seconds(1);
        assert_eq!(season_reminder_invalid_reason(before, boundary(), 2), None);
        assert_eq!(
            season_reminder_invalid_reason(boundary(), boundary(), 2),
            Some("season_boundary_passed")
        );
        assert_eq!(
            season_reminder_invalid_reason(before, boundary(), 0),
            Some("season_reminder_resolved")
        );
    }
}
