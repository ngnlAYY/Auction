//! 卖方成交汇总：每个卖方每场一封，正文由已结算事实冻结。
//!
//! 汇总只引用已持久化的场次、lot 代码、装备名与最终成交价；不读取配置或时钟。
//! 同一 (auction, seller) 必须产生同一身份与正文，扫描重复执行才能保持幂等。

use std::collections::BTreeMap;

use auction_domain::CommandId;
use uuid::Uuid;

use super::valid_text;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::RequestHash;
use crate::ports::mail::{
    MailDeliveryCommand, MailDeliveryOperation, MailDeliveryOrigin, MailNotificationTemplate,
};
use crate::ports::player_notifications::{PlayerNotificationError, SellerSummaryLotFact};
use crate::services::public::{format_credits, lot_code};

/// 汇总正文上限；超出时不截断结算结果，直接不创建通知。
pub const MAX_SELLER_SUMMARY_BODY_BYTES: usize = 16 << 10;

/// 正文最多列出的 lot 数；其余只给计数，避免单封邮件无限增长。
pub const MAX_SELLER_SUMMARY_LOTS: usize = 64;

/// 汇总的固定主题；正文才是逐卖方冻结内容。
pub const SELLER_SUMMARY_SUBJECT: &str = "Auction session results";

/// 单个 lot 的结算结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SellerSummaryLot {
    /// 场次内稳定序号；只用于排序/去重。
    pub lot_number: u32,
    /// 玩家显示的类别内编号。
    pub category_number: u32,
    pub category: String,
    pub equipment_name: String,
    /// `None` 表示流拍。
    pub sold_price: Option<i64>,
}

/// 一个卖方在一场结算中的完整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellerSummary {
    pub auction_id: Uuid,
    pub world: String,
    pub sequence: i64,
    pub seller: String,
    pub lots: Vec<SellerSummaryLot>,
}

impl SellerSummary {
    /// 该卖方在此场次中成交的 lot 数。
    pub fn sold_count(&self) -> usize {
        self.lots.iter().filter(|lot| lot.sold_price.is_some()).count()
    }

    /// 该卖方在此场次中流拍的 lot 数。
    pub fn unsold_count(&self) -> usize {
        self.lots.len() - self.sold_count()
    }

    /// 成交价合计；流拍不计入。
    pub fn sold_total(&self) -> i64 {
        self.lots.iter().filter_map(|lot| lot.sold_price).sum()
    }
}

/// 按 (auction, seller) 合并扫描事实；结果按身份排序，保证重复扫描顺序稳定。
pub fn group_seller_summaries(
    facts: Vec<SellerSummaryLotFact>,
) -> Result<Vec<SellerSummary>, PlayerNotificationError> {
    let mut grouped: BTreeMap<(Uuid, String), SellerSummary> = BTreeMap::new();
    for fact in facts {
        if fact.auction_id.is_nil()
            || !matches!(fact.world.as_str(), "main" | "isekai")
            || fact.sequence <= 0
            || !valid_text(&fact.seller, 256)
            || fact.lot_number == 0
            || fact.category_number == 0
            || !valid_text(&fact.category, 128)
            || !valid_text(&fact.equipment_name, 512)
            || fact.sold_price.is_some_and(|price| price <= 0)
        {
            return Err(PlayerNotificationError::InvalidSellerSummary);
        }
        let key = (fact.auction_id, fact.seller.clone());
        let summary = grouped.entry(key).or_insert_with(|| SellerSummary {
            auction_id: fact.auction_id,
            world: fact.world.clone(),
            sequence: fact.sequence,
            seller: fact.seller.clone(),
            lots: Vec::new(),
        });
        // 同一场次的场次号必须一致；不一致说明扫描投影损坏，不能合并。
        if summary.world != fact.world || summary.sequence != fact.sequence {
            return Err(PlayerNotificationError::InvalidSellerSummary);
        }
        summary.lots.push(SellerSummaryLot {
            lot_number: fact.lot_number,
            category_number: fact.category_number,
            category: fact.category,
            equipment_name: fact.equipment_name,
            sold_price: fact.sold_price,
        });
    }
    let mut summaries = grouped.into_values().collect::<Vec<_>>();
    for summary in &mut summaries {
        summary.lots.sort_by_key(|lot| lot.lot_number);
        if summary.lots.windows(2).any(|pair| pair[0].lot_number == pair[1].lot_number) {
            return Err(PlayerNotificationError::InvalidSellerSummary);
        }
    }
    Ok(summaries)
}

/// 由已合并事实生成冻结正文；超出正文预算返回 `None`，不截断结果。
pub fn seller_summary_body(summary: &SellerSummary) -> Option<String> {
    let mut body = String::with_capacity(160 + summary.lots.len() * 96);
    body.push_str("Your auction session has been settled.\n\n");
    body.push_str(&format!("Session: {} #{}\n\n", summary.world, summary.sequence));
    body.push_str("Sold:\n");
    let mut listed = 0_usize;
    for lot in summary.lots.iter().filter(|lot| lot.sold_price.is_some()) {
        if listed >= MAX_SELLER_SUMMARY_LOTS {
            break;
        }
        listed += 1;
        let price = lot.sold_price.unwrap_or_default();
        body.push_str(&format!(
            "- {}: {} - {} Credits\n",
            lot_code(&lot.category, lot.category_number),
            lot.equipment_name,
            format_credits(price)
        ));
    }
    if listed == 0 {
        body.push_str("- none\n");
    }
    body.push_str("\nUnsold:\n");
    let mut unsold_listed = 0_usize;
    for lot in summary.lots.iter().filter(|lot| lot.sold_price.is_none()) {
        if unsold_listed >= MAX_SELLER_SUMMARY_LOTS {
            break;
        }
        unsold_listed += 1;
        body.push_str(&format!(
            "- {}: {}\n",
            lot_code(&lot.category, lot.category_number),
            lot.equipment_name
        ));
    }
    if unsold_listed == 0 {
        body.push_str("- none\n");
    }
    let hidden = summary.lots.len().saturating_sub(listed + unsold_listed);
    if hidden > 0 {
        body.push_str(&format!("\nAnd {hidden} more lots not listed here.\n"));
    }
    body.push_str(&format!(
        "\nTotals: {} sold, {} unsold, {} Credits.\n",
        summary.sold_count(),
        summary.unsold_count(),
        format_credits(summary.sold_total())
    ));
    body.push_str("\nPayout and unsold returns are sent separately.");
    (body.len() <= MAX_SELLER_SUMMARY_BODY_BYTES).then_some(body)
}

/// 生成稳定入队命令；同一 (auction, seller) 重放得到同一 command_id 与请求摘要。
pub fn seller_summary_command(
    summary: &SellerSummary,
    body: String,
) -> Result<MailDeliveryCommand, PlayerNotificationError> {
    let notification_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/seller-summary-v1/{}/{}", summary.auction_id, summary.seller).as_bytes(),
    );
    // 卖方货款在 main 支付，结算通知也统一寄往 main，避免 isekai 换季关服后无法收取。
    let origin_id = format!("seller_summary:{}:{}", summary.auction_id, summary.seller);
    let request_hash = RequestHash::from_request(
        "seller_summary_notification",
        notification_id,
        &(
            summary.auction_id,
            &summary.world,
            summary.sequence,
            &summary.seller,
            &summary.lots,
            &body,
        ),
    )
    .map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "seller_summary",
            "hash_notification",
            DiagnosticContext::default(),
        );
        PlayerNotificationError::Unavailable
    })?;
    Ok(MailDeliveryCommand {
        command_id: CommandId::from_uuid(notification_id),
        request_hash,
        origin: MailDeliveryOrigin::SellerSummary,
        origin_id,
        operation: MailDeliveryOperation::Send,
        world_id: "main".to_owned(),
        external_mail_id: notification_id.to_string(),
        recipient: summary.seller.clone(),
        template: Some(MailNotificationTemplate::AuctionSellerSummary),
        result_code: Some("summary".to_owned()),
        user_key_user_id: None,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: Some(body),
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn auction() -> Uuid {
        Uuid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888)
    }

    fn fact(seller: &str, lot_number: u32, sold_price: Option<i64>) -> SellerSummaryLotFact {
        SellerSummaryLotFact {
            auction_id: auction(),
            world: "isekai".to_owned(),
            sequence: 7,
            seller: seller.to_owned(),
            lot_number,
            category_number: lot_number,
            category: "Cloth".to_owned(),
            equipment_name: format!("Item {lot_number}"),
            sold_price,
        }
    }

    #[test]
    fn groups_every_lot_of_one_seller_into_a_single_summary() {
        let summaries = group_seller_summaries(vec![
            fact("alice", 2, None),
            fact("bob", 1, Some(70_000)),
            fact("alice", 1, Some(50_000)),
        ])
        .expect("合法事实必须聚合");
        assert_eq!(summaries.len(), 2);
        let alice =
            summaries.iter().find(|summary| summary.seller == "alice").expect("alice 汇总必须存在");
        assert_eq!(alice.lots.iter().map(|lot| lot.lot_number).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(alice.sold_count(), 1);
        assert_eq!(alice.unsold_count(), 1);
        assert_eq!(alice.sold_total(), 50_000);
    }

    #[test]
    fn summary_body_lists_codes_amounts_and_totals() {
        let summary =
            group_seller_summaries(vec![fact("alice", 1, Some(50_000)), fact("alice", 2, None)])
                .expect("合法事实必须聚合")
                .remove(0);
        let body = seller_summary_body(&summary).expect("正文必须可生成");
        assert!(body.contains("Session: isekai #7"), "{body}");
        assert!(body.contains("Clo01"), "{body}");
        assert!(body.contains("50,000 Credits"), "{body}");
        assert!(body.contains("Clo02"), "{body}");
        assert!(body.contains("Totals: 1 sold, 1 unsold, 50,000 Credits."), "{body}");
    }

    #[test]
    fn summary_identity_is_stable_and_one_per_seller_and_auction() {
        let summaries =
            group_seller_summaries(vec![fact("alice", 1, Some(50_000)), fact("alice", 2, None)])
                .expect("合法事实必须聚合");
        let summary = &summaries[0];
        let body = seller_summary_body(summary).expect("正文必须可生成");
        let first = seller_summary_command(summary, body.clone()).expect("命令必须生成");
        let second = seller_summary_command(summary, body).expect("命令必须生成");
        assert_eq!(first.command_id, second.command_id);
        assert_eq!(first.request_hash, second.request_hash);
        assert_eq!(first.origin, MailDeliveryOrigin::SellerSummary);
        assert_eq!(first.origin_id, format!("seller_summary:{}:alice", auction()));
        assert_eq!(first.world_id, "main");
    }

    #[test]
    fn display_code_uses_category_number_not_auction_ordinal() {
        let mut first = fact("alice", 17, Some(1_000));
        first.category_number = 1;
        let mut second = fact("alice", 18, Some(2_000));
        second.category_number = 2;
        let summary =
            group_seller_summaries(vec![first, second]).expect("合法事实必须聚合").remove(0);
        let body = seller_summary_body(&summary).expect("正文必须可生成");
        assert!(body.contains("Clo01"), "{body}");
        assert!(body.contains("Clo02"), "{body}");
        assert!(!body.contains("Clo17"), "{body}");
        assert!(!body.contains("Clo18"), "{body}");
    }

    #[test]
    fn duplicate_lot_numbers_are_rejected_instead_of_merged() {
        let error =
            group_seller_summaries(vec![fact("alice", 1, Some(50_000)), fact("alice", 1, None)])
                .expect_err("同一 lot 不能出现两次");
        assert_eq!(error, PlayerNotificationError::InvalidSellerSummary);
    }
}
