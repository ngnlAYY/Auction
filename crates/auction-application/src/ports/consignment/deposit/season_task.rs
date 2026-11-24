//! 季窗冻结与季前通知投递的窄端口。
//!
//! Worker 只经本端口读取已冻结季窗、待通知 position 与待建投递的通知；季窗一旦冻结，
//! 同一 `(world, season_id)` 的界值不再改变（不变量 10）。投递、工作与通知的派生身份
//! 也在这里定义一次，存储适配器与测试共用。

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::FrozenSeason;
use auction_time::SecondsFormat;
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::consignment::errors::ConsignmentError;
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;
use crate::ports::mail::MailDeliveryCommand;
use crate::ports::mail::MailDeliveryOperation;
use crate::ports::mail::MailDeliveryOrigin;
use crate::ports::mail::MailNotificationTemplate;

/// 寄售季前通知的固定英文主题。
pub const CONSIGNMENT_SEASON_NOTICE_SUBJECT: &str = "Isekai consignment season notice";

/// 通知正文结果码；与模板绑定一起冻结。
pub const CONSIGNMENT_SEASON_NOTICE_RESULT_CODE: &str = "season_preclose";

/// 冻结季窗内仍需要收到季前撤回通知的 position。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeasonNoticeTarget {
    /// 代管 position。
    pub position_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
}

/// 已有季前通知但尚未建立 Send 投递的卖家。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonNoticeDelivery {
    /// 通知 UUID。
    pub notice_id: Uuid,
    /// 代管 position。
    pub position_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
    /// isekai 角色名，作为收件人。
    pub seller_name: String,
}

/// 同一事务里写投递行与 `action='send'` 工作行的命令。
#[derive(Debug, Clone)]
pub struct SeasonNoticeDeliveryCommand {
    /// 通用投递命令；投递 ID 由 [`season_notice_delivery_id`] 显式给出。
    pub delivery: MailDeliveryCommand,
    /// 通知 UUID。
    pub notice_id: Uuid,
    /// `mail_disposition_work.id`。
    pub work_id: Uuid,
}

/// 季窗冻结、通知扫描与季末关闭的持久事实来源。
pub trait SeasonTaskStore: Send + Sync {
    /// 首次冻结该 `(world, season_id)`；已有冻结行时原样返回，不改写界值。
    ///
    /// `timezone` 只来自日历 owner，用于持久化展示时区；不参与季界计算。
    fn freeze_season_window<'a>(
        &'a self,
        season: FrozenSeason,
        timezone: &'a str,
    ) -> ConsignmentFuture<'a, FrozenSeason>;

    /// 全部已冻结季窗，按季末升序。
    fn frozen_seasons(&self) -> ConsignmentFuture<'_, Vec<FrozenSeason>>;

    /// 预停已到、季末未到，且尚无 `preclose_withdrawal` 通知的非终态 position。
    fn season_notice_targets<'a>(
        &'a self,
        season: &'a FrozenSeason,
    ) -> ConsignmentFuture<'a, Vec<SeasonNoticeTarget>>;

    /// 已有 `preclose_withdrawal` 通知但还没有工作行/投递的卖家。
    fn season_notice_deliveries<'a>(
        &'a self,
        season: &'a FrozenSeason,
    ) -> ConsignmentFuture<'a, Vec<SeasonNoticeDelivery>>;

    /// 同一事务写入通用投递与 `action='send'` 工作；重复调用按派生身份幂等。
    fn create_season_notice_delivery(
        &self,
        command: SeasonNoticeDeliveryCommand,
    ) -> ConsignmentFuture<'_, ()>;

    /// 该冻结季窗是否仍有登记早于季末的非终态 isekai 挂单。
    fn season_close_pending<'a>(&'a self, season: &'a FrozenSeason) -> ConsignmentFuture<'a, bool>;
}

/// 投递与投递命令 ID 由 `notice_id` 派生的唯一身份。
pub fn season_notice_delivery_id(notice_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/consignment-notice-delivery/{notice_id}").as_bytes(),
    )
}

/// Send 工作 ID 由 `notice_id` 派生的唯一身份。
pub fn season_notice_work_id(notice_id: Uuid) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/consignment-notice-work/{notice_id}").as_bytes(),
    )
}

/// 通用投递的外壳身份。
pub fn season_notice_external_mail_id(notice_id: Uuid) -> String {
    format!("consignment-notice:{notice_id}")
}

/// 通用投递的来源身份。
pub fn season_notice_origin_id(notice_id: Uuid) -> String {
    format!("consignment_notice:{notice_id}")
}

/// 冻结正文：只含季末 UTC 时刻与撤回入口，不含预约码、装备 key 或任何凭据。
pub fn season_notice_body(season: &FrozenSeason) -> String {
    format!(
        "Your Isekai consignment items will be closed when the season ends at {}.\n\nIf you want to withdraw them before then, open the consignment page on the auction website and request a withdrawal.",
        season.end_utc.to_rfc3339_opts(SecondsFormat::AutoSi, true)
    )
}

/// 由已提交的通知生成通用 Send 投递命令；同一通知重放得到同一命令与摘要。
pub fn season_notice_delivery_command(
    season: &FrozenSeason,
    notice_id: Uuid,
    seller_name: &str,
) -> Result<SeasonNoticeDeliveryCommand, ConsignmentError> {
    if season.world != SettlementWorld::Isekai
        || notice_id.is_nil()
        || !auction_domain::valid_text(seller_name, 256)
    {
        return Err(ConsignmentError::Unavailable);
    }
    let body = season_notice_body(season);
    let delivery_id = season_notice_delivery_id(notice_id);
    let request_hash = RequestHash::from_request(
        "consignment_season_notice",
        delivery_id,
        &(season.season_id.as_str(), season.end_utc.to_rfc3339(), body.as_str()),
    )
    .map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "consignment_season_notice",
            "hash_notification",
            DiagnosticContext::default(),
        );
        ConsignmentError::Unavailable
    })?;
    Ok(SeasonNoticeDeliveryCommand {
        delivery: MailDeliveryCommand {
            command_id: CommandId::from_uuid(delivery_id),
            request_hash,
            origin: MailDeliveryOrigin::ConsignmentNotice,
            origin_id: season_notice_origin_id(notice_id),
            operation: MailDeliveryOperation::Send,
            world_id: "isekai".to_owned(),
            external_mail_id: season_notice_external_mail_id(notice_id),
            recipient: seller_name.to_owned(),
            template: Some(MailNotificationTemplate::ConsignmentSeasonNotice),
            result_code: Some(CONSIGNMENT_SEASON_NOTICE_RESULT_CODE.to_owned()),
            user_key_user_id: None,
            credits: 0,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: Some(body),
        },
        notice_id,
        work_id: season_notice_work_id(notice_id),
    })
}

#[cfg(test)]
mod tests {
    use auction_domain::SettlementWorld;
    use auction_domain::consignment::{CalendarVersion, FrozenSeason};
    use auction_time::Timestamp;

    use super::{
        season_notice_body, season_notice_delivery_command, season_notice_delivery_id,
        season_notice_work_id,
    };

    fn season() -> FrozenSeason {
        FrozenSeason {
            world: SettlementWorld::Isekai,
            season_id: "2026-11-01".to_owned(),
            start_utc: Timestamp::parse_from_rfc3339("2026-05-01T09:00:00Z").expect("开始"),
            end_utc: Timestamp::parse_from_rfc3339("2026-11-01T09:00:00Z").expect("结束"),
            admission_cutoff_utc: Timestamp::parse_from_rfc3339("2026-10-29T00:00:00Z")
                .expect("预停"),
            calendar_version: CalendarVersion::from_bytes([7; 32]),
            preclose_days: 3,
        }
    }

    #[test]
    fn notice_command_is_deterministic_and_derives_from_the_notice() {
        let notice_id = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let first = season_notice_delivery_command(&season(), notice_id, "FakeSeller")
            .expect("命令必须生成");
        let second = season_notice_delivery_command(&season(), notice_id, "FakeSeller")
            .expect("重放命令必须生成");
        assert_eq!(first.delivery.command_id, second.delivery.command_id);
        assert_eq!(first.delivery.command_id.as_uuid(), season_notice_delivery_id(notice_id));
        assert_eq!(first.work_id, season_notice_work_id(notice_id));
        assert_eq!(first.delivery.external_mail_id, format!("consignment-notice:{notice_id}"));
        assert_eq!(first.delivery.origin_id, format!("consignment_notice:{notice_id}"));
        let body = season_notice_body(&season());
        assert!(body.contains("2026-11-01T09:00:00Z"));
        assert!(body.contains("withdraw"));
        assert!(!body.contains("C1-"));
    }
}
