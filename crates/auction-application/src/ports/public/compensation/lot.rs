//! Public 逐件结算明细与成交撤销冲正的模型和一致性校验。
//!
//! 每个已售 lot 公开成交价、持有/调整比例与结算金额，玩家可据此复算附加费与补偿；
//! 冲正事实只追加，存在时说明该件成交已撤销且池余额已按冻结规则回滚。

use super::*;
use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};

/// 单场逐件明细的编号分页与补偿账本分页共用同一规则。
pub type PublicCompensationLotRequest = PublicCompensationMovementRequest;

/// 成交撤销后对池余额的冲正。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationLotReversal {
    /// 对池余额的带符号净变化：退回补偿减去冲回附加费。
    pub amount: i64,
    /// 余额不足、未能冲回的附加费。
    pub tax_shortfall: i64,
    pub balance_after: i64,
    pub created_at: DateTime<Utc>,
}

/// 单个已售 lot 的公开结算明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationLot {
    pub lot_id: Uuid,
    pub lot_number: i32,
    pub equipment_name: String,
    pub equipment_category: String,
    /// 仅用于解析游戏装备链接，不进入 Public DTO。
    pub equipment_id: auction_domain::EquipmentId,
    /// main/isekai 的权威装备 URL 前缀投影；不进入 Public DTO。
    pub projected_game_equipment_url: String,
    pub end_price: i64,
    pub hold_ppm: i32,
    pub adjustment_ppm: i32,
    pub leader_held_nanos: i64,
    pub tax_amount: i64,
    /// 名义优惠需求：成交价与优惠后挂牌价之差。
    pub discount_amount: i64,
    pub compensation_amount: i64,
    pub buyer_amount: i64,
    pub seller_amount: i64,
    /// 买家当前玩家名，与成交历史同一口径。
    pub buyer_name: String,
    /// 投稿卖家名；没有投稿记录的历史 lot 为 `—`，与成交历史同一口径。
    pub seller_name: String,
    /// 成交是否已被恢复流程撤销；升级前撤销的成交可能没有冲正事实。
    pub voided: bool,
    pub reversal: Option<PublicCompensationLotReversal>,
}

impl PublicCompensationLot {
    /// 校验金额守恒 `买方实付 + 补偿 = 卖方所得 + 附加费`、比例范围与冲正一致性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.lot_id.is_nil()
            || self.lot_number <= 0
            || !auction_domain::valid_text(&self.equipment_name, 256)
            || !auction_domain::valid_text(&self.equipment_category, 64)
            || self.equipment_id.is_nil()
            || !valid_projected_equipment_url(&self.projected_game_equipment_url)
            || self.end_price <= 0
            || !(0..=1_000_000).contains(&self.hold_ppm)
            || !(-1_000_000..=1_000_000).contains(&self.adjustment_ppm)
            || self.leader_held_nanos < 0
            || self.tax_amount < 0
            || self.discount_amount < 0
            || self.discount_amount > self.end_price
            || self.compensation_amount < 0
            || self.buyer_amount < 0
            || self.seller_amount < 0
            || !auction_domain::valid_text(&self.buyer_name, 256)
            || !auction_domain::valid_text(&self.seller_name, 256)
        {
            return Err("公开逐件结算明细无效");
        }
        let paid = self.buyer_amount.checked_add(self.compensation_amount);
        let received = self.seller_amount.checked_add(self.tax_amount);
        if paid.is_none() || paid != received {
            return Err("公开逐件结算金额不守恒");
        }
        if let Some(reversal) = self.reversal.as_ref() {
            let tax_reversed = self.tax_amount.checked_sub(reversal.tax_shortfall);
            if !self.voided
                || (self.tax_amount == 0 && self.compensation_amount == 0)
                || reversal.tax_shortfall < 0
                || reversal.tax_shortfall > self.tax_amount
                || reversal.balance_after < 0
                || (reversal.tax_shortfall > 0 && reversal.balance_after != 0)
                || tax_reversed.and_then(|value| self.compensation_amount.checked_sub(value))
                    != Some(reversal.amount)
                || reversal.created_at.timestamp_micros() == 0
            {
                return Err("公开成交撤销冲正无效");
            }
        }
        Ok(())
    }
}

/// 绑定同一快照读取的补偿摘要与逐件明细页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCompensationLotPage {
    pub round: PublicCompensationRound,
    pub entries: Vec<PublicCompensationLot>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl PublicCompensationLotPage {
    /// 校验摘要、分页范围、逐件明细与本页冲正笔数上界。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.round.validate()?;
        PublicCompensationLotRequest { limit: self.limit, offset: self.offset }.validate()?;
        let entry_count = i64::try_from(self.entries.len()).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "compensation_lot",
                "convert_entry_count",
                DiagnosticContext::default(),
            );
            "公开逐件结算明细页大小溢出"
        })?;
        let reversal_count = self.entries.iter().filter(|entry| entry.reversal.is_some()).count();
        if self.total < entry_count
            || entry_count > self.limit
            || i64::try_from(reversal_count).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "compensation_lot",
                    "convert_reversal_count",
                    DiagnosticContext::default(),
                );
                "公开逐件结算明细页大小溢出"
            })? > self.round.reversal_count
        {
            return Err("公开逐件结算明细页无效");
        }
        for (index, entry) in self.entries.iter().enumerate() {
            entry.validate()?;
            if self.entries[..index].iter().any(|other| other.lot_id == entry.lot_id) {
                return Err("公开逐件结算明细重复");
            }
        }
        Ok(())
    }
}
