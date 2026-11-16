//! Public 履约进度读取端口：已结算场次逐件的结算后流程事实与闭集公开状态。
//!
//! 存储适配器只在同一只读快照中取事实（lot 终态、结算明细、各义务的履约与投递状态原值、
//! recovery 与装备损失），由 [`public_fulfillment_view`] 把事实映射为闭集状态；HTTP 只把状态
//! 映射为 wire 字符串，浏览器只把字符串映射为文案。邮件、投递、履约与命令标识、诊断码、
//! Bot 身份与代理模式都不进入本端口的公开输出。

use auction_domain::SettlementWorld;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::ApplicationFuture;

mod facts;
mod status;

pub use facts::{PublicFulfillmentLot, PublicFulfillmentObligation, PublicFulfillmentRecovery};
pub use status::{
    PublicFulfillmentCodReturns, PublicFulfillmentSale, PublicFulfillmentStatus,
    PublicFulfillmentView, public_fulfillment_view,
};

/// 履约进度编号页的默认大小。
pub const DEFAULT_PUBLIC_FULFILLMENT_PAGE_SIZE: i64 = 50;

/// 履约进度编号页的最大大小；新路由首发即设上限，超出由 HTTP 按查询无效拒绝。
pub const MAX_PUBLIC_FULFILLMENT_PAGE_SIZE: i64 = 100;

/// 履约进度编号页允许的最大 offset。
pub const MAX_PUBLIC_FULFILLMENT_OFFSET: i64 = 10_000;

/// 单个已结算场次的履约进度查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentRequest {
    pub auction_id: Uuid,
    /// 可选的世界约束；与场次世界不符时按不可见处理。
    pub world_id: Option<String>,
    /// lot UUID 精确筛选；与 lot 代码筛选互斥。
    pub lot_id: Option<Uuid>,
    /// 人类可读 lot 代码的大小写不敏感包含匹配，口径与竞价日志一致。
    pub lot_code: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl PublicFulfillmentRequest {
    /// 在端口边界校验资源标识、筛选文本与编号页范围。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || self
                .world_id
                .as_deref()
                .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
            || self.lot_id.is_some_and(|lot_id| lot_id.is_nil())
            || self.lot_id.is_some() && self.lot_code.is_some()
            || self.lot_code.as_deref().is_some_and(|lot_code| {
                lot_code.is_empty()
                    || lot_code.len() > 32
                    || !lot_code.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
            || !(1..=MAX_PUBLIC_FULFILLMENT_PAGE_SIZE).contains(&self.limit)
            || !(0..=MAX_PUBLIC_FULFILLMENT_OFFSET).contains(&self.offset)
        {
            return Err("公开履约进度查询参数无效");
        }
        Ok(())
    }
}

/// 履约进度所属的已结算场次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentAuction {
    pub auction_id: Uuid,
    pub world_id: String,
    pub sequence: i64,
}

/// 同一只读快照读取的场次、读取时刻、总数与本页逐件事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentPage {
    pub auction: PublicFulfillmentAuction,
    /// 快照内的数据库读取时刻；付款逾期按它判定，同一页所有 lot 共用同一时刻。
    pub read_at: DateTime<Utc>,
    pub entries: Vec<PublicFulfillmentLot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl PublicFulfillmentPage {
    /// 校验场次、分页范围与逐件事实，并确认每件都能映射为闭集状态：任一未知原值都让整页
    /// 失败关闭，不把无法判定的事实显示成某个正常阶段。场次世界在页级按闭集校验：空页（筛选
    /// 无匹配或越过末页）不经过逐件判定，世界不在闭集内时同样失败关闭。
    pub fn validate(&self) -> Result<(), &'static str> {
        let entry_count = i64::try_from(self.entries.len()).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "public_fulfillment",
                "convert_entry_count",
                DiagnosticContext::default(),
            );
            "公开履约进度页大小溢出"
        })?;
        if self.auction.auction_id.is_nil()
            || SettlementWorld::from_name(&self.auction.world_id).is_none()
            || self.auction.sequence <= 0
            || self.read_at.timestamp_micros() == 0
            || !(1..=MAX_PUBLIC_FULFILLMENT_PAGE_SIZE).contains(&self.limit)
            || !(0..=MAX_PUBLIC_FULFILLMENT_OFFSET).contains(&self.offset)
            || entry_count > self.limit
            || self.total < entry_count
            || entry_count > 0
                && self.offset.checked_add(entry_count).is_none_or(|end| end > self.total)
        {
            return Err("公开履约进度页无效");
        }
        for (index, entry) in self.entries.iter().enumerate() {
            if self.entries[..index].iter().any(|other| other.lot_id == entry.lot_id) {
                return Err("公开履约进度 lot 重复");
            }
        }
        self.views().map(|_| ())
    }

    /// 按页内顺序给出每件的公开状态与附带字段；读取时刻与场次世界对整页一致。
    pub fn views(&self) -> Result<Vec<PublicFulfillmentView>, &'static str> {
        self.entries
            .iter()
            .map(|lot| public_fulfillment_view(lot, &self.auction.world_id, self.read_at))
            .collect()
    }
}

/// Public 履约进度只读端口。
pub trait PublicFulfillmentReader: Send + Sync {
    /// 在同一只读快照中读取已结算场次的总数与本页逐件事实；场次不存在、未结算、不可见或与
    /// 世界约束不符时返回 `None`。
    fn list_public_fulfillment(
        &self,
        request: PublicFulfillmentRequest,
    ) -> ApplicationFuture<'_, Option<PublicFulfillmentPage>>;
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration};
    use uuid::Uuid;

    use super::{
        MAX_PUBLIC_FULFILLMENT_OFFSET, MAX_PUBLIC_FULFILLMENT_PAGE_SIZE, PublicFulfillmentAuction,
        PublicFulfillmentLot, PublicFulfillmentObligation, PublicFulfillmentPage,
        PublicFulfillmentRecovery, PublicFulfillmentRequest, PublicFulfillmentStatus,
    };

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);

    fn request() -> PublicFulfillmentRequest {
        PublicFulfillmentRequest {
            auction_id: AUCTION_ID,
            world_id: None,
            lot_id: None,
            lot_code: None,
            limit: 50,
            offset: 0,
        }
    }

    fn unsold_lot(id: u128) -> PublicFulfillmentLot {
        PublicFulfillmentLot {
            lot_id: Uuid::from_u128(id),
            lot_number: 1,
            equipment_name: "Average Cotton Cap".to_owned(),
            equipment_category: "Cloth".to_owned(),
            seller_name: "seller".to_owned(),
            lot_status: "unsold".to_owned(),
            buyer_name: None,
            final_price: Some(0),
            equipment_lost: false,
            recovery: None,
            obligations: vec![PublicFulfillmentObligation {
                kind: "unsold_return".to_owned(),
                business_state: "ready".to_owned(),
                execution_state: Some("pending".to_owned()),
                delivery_status: None,
            }],
        }
    }

    fn page(entries: Vec<PublicFulfillmentLot>, total: i64) -> PublicFulfillmentPage {
        PublicFulfillmentPage {
            auction: PublicFulfillmentAuction {
                auction_id: AUCTION_ID,
                world_id: "main".to_owned(),
                sequence: 12,
            },
            read_at: DateTime::from_timestamp(1_763_200_000, 0).expect("固定读取时刻"),
            entries,
            total,
            limit: 50,
            offset: 0,
        }
    }

    #[test]
    fn request_caps_page_size_and_offset() {
        assert_eq!(request().validate(), Ok(()));
        for (limit, offset) in
            [(MAX_PUBLIC_FULFILLMENT_PAGE_SIZE, MAX_PUBLIC_FULFILLMENT_OFFSET), (1, 0)]
        {
            assert_eq!(PublicFulfillmentRequest { limit, offset, ..request() }.validate(), Ok(()));
        }
        for (limit, offset) in [
            (0, 0),
            (MAX_PUBLIC_FULFILLMENT_PAGE_SIZE + 1, 0),
            (50, -1),
            (50, MAX_PUBLIC_FULFILLMENT_OFFSET + 1),
        ] {
            assert!(PublicFulfillmentRequest { limit, offset, ..request() }.validate().is_err());
        }
    }

    #[test]
    fn request_lot_filters_match_the_bid_log_rules() {
        let lot_id = Some(Uuid::from_u128(7));
        let code = |value: &str| Some(value.to_owned());
        assert_eq!(PublicFulfillmentRequest { lot_id, ..request() }.validate(), Ok(()));
        assert_eq!(
            PublicFulfillmentRequest { lot_code: code("Clo12"), ..request() }.validate(),
            Ok(())
        );
        for invalid in [
            PublicFulfillmentRequest { lot_id, lot_code: code("Clo12"), ..request() },
            PublicFulfillmentRequest { lot_id: Some(Uuid::nil()), ..request() },
            PublicFulfillmentRequest { lot_code: code(""), ..request() },
            PublicFulfillmentRequest { lot_code: code("Clo-12"), ..request() },
            PublicFulfillmentRequest { lot_code: Some("a".repeat(33)), ..request() },
            PublicFulfillmentRequest { world_id: code(" main"), ..request() },
            PublicFulfillmentRequest { auction_id: Uuid::nil(), ..request() },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn page_views_keep_the_page_order() {
        let page = page(vec![unsold_lot(1), unsold_lot(2)], 2);
        assert_eq!(page.validate(), Ok(()));
        let statuses = page.views().expect("可判定").into_iter().map(|view| view.status);
        assert_eq!(statuses.collect::<Vec<_>>(), [PublicFulfillmentStatus::UnsoldReturnPending; 2]);
    }

    #[test]
    fn page_rejects_inconsistent_bounds_duplicates_and_unknown_facts() {
        assert!(page(vec![unsold_lot(1), unsold_lot(2)], 1).validate().is_err(), "总数小于本页");
        assert!(page(vec![unsold_lot(1), unsold_lot(1)], 2).validate().is_err(), "lot 重复");
        let mut beyond = page(vec![unsold_lot(1)], 1);
        beyond.offset = 50;
        assert!(beyond.validate().is_err(), "本页越过总数");
        let mut empty_tail = page(Vec::new(), 1);
        empty_tail.offset = 50;
        assert_eq!(empty_tail.validate(), Ok(()), "越过末页的空页仍有效");
        let mut unknown = unsold_lot(1);
        unknown.obligations[0].execution_state = Some("archived".to_owned());
        assert!(page(vec![unknown], 1).validate().is_err(), "未知原值整页失败关闭");
        let mut unknown_world = page(vec![unsold_lot(1)], 1);
        unknown_world.auction.world_id = "persistent".to_owned();
        assert!(unknown_world.validate().is_err(), "场次世界不在闭集内");
        // 空页不经过逐件判定，世界闭集仍在页级失败关闭。
        for (total, offset) in [(0, 0), (1, 50)] {
            let mut empty = page(Vec::new(), total);
            empty.offset = offset;
            empty.auction.world_id = "persistent".to_owned();
            assert!(empty.validate().is_err(), "未知世界的空页 total={total}");
            empty.auction.world_id = "isekai".to_owned();
            assert_eq!(empty.validate(), Ok(()), "闭集内世界的空页 total={total}");
        }
        let mut too_many = page(vec![unsold_lot(1), unsold_lot(2)], 2);
        too_many.limit = 1;
        assert!(too_many.validate().is_err(), "本页超过页大小");
        let mut stale = page(Vec::new(), 0);
        stale.read_at = DateTime::from_timestamp(0, 0).expect("零时刻");
        assert!(stale.validate().is_err(), "读取时刻缺失");
    }

    #[test]
    fn recovery_shape_is_checked_before_classification() {
        let mut lot = unsold_lot(1);
        lot.lot_status = "sold".to_owned();
        lot.buyer_name = Some("buyer".to_owned());
        lot.final_price = Some(10);
        lot.recovery = Some(PublicFulfillmentRecovery {
            stage: "awaiting_payment".to_owned(),
            resolution: None,
            payment_deadline_at: DateTime::from_timestamp(1_763_200_000, 0).expect("期限")
                + Duration::days(1),
            cod_return_count: 0,
            cod_return_limit: 0,
            blocking_code: None,
            manual_reconcile: false,
            relisted: false,
        });
        assert!(page(vec![lot], 1).validate().is_err(), "冻结上限至少为 1");
    }
}
