//! Sale 合同条款与成交时冻结的恢复策略。

use auction_time::Timestamp;
use uuid::Uuid;

use super::errors::ConsignmentContractError;
use crate::consignment::{
    FeeBps, ListingPrice, ListingQuote, ListingVersion, PRICING_VERSION, quote_v1,
};
use crate::{UserId, valid_text};

/// 结算策略版本；改变任何冻结范围或含义必须以新版本表达，已冻结合同不改写。
pub const CONSIGNMENT_SETTLEMENT_POLICY_VERSION: u32 = 1;

/// 冻结身份文本的字节上限，与合同表的文本约束一致。
pub const MAX_FROZEN_TEXT_BYTES: usize = 128;

/// 买家提醒次数的固定常量。
pub const REMINDER_LIMIT: u8 = 1;
/// 异世界拒付统计窗口的固定天数。
pub const ISEKAI_REFUSAL_WINDOW_DAYS: u16 = 90;

/// 成交时冻结的恢复策略。
///
/// `reminder_limit` 与 `isekai_refusal_window_days` 是固定常量，仍落库，以便以后改策略时用新
/// 版本表达而不是改旧合同（不变量 10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsignmentSalePolicy {
    pub payment_deadline_days: u16,
    pub reminder_limit: u8,
    pub cod_return_limit: u8,
    pub isekai_refusal_limit: u8,
    pub isekai_refusal_window_days: u16,
}

impl ConsignmentSalePolicy {
    /// 付款期限天数范围。
    pub const PAYMENT_DEADLINE_DAYS: std::ops::RangeInclusive<u16> = 1..=90;
    /// COD 退回上限范围。
    pub const COD_RETURN_LIMIT: std::ops::RangeInclusive<u8> = 2..=10;
    /// 异世界拒付上限范围。
    pub const ISEKAI_REFUSAL_LIMIT: std::ops::RangeInclusive<u8> = 2..=100;

    /// 只接受配置范围内的取值，两个固定常量由本函数写入；越界失败关闭，不截断。
    pub fn new(
        payment_deadline_days: u16,
        cod_return_limit: u8,
        isekai_refusal_limit: u8,
    ) -> Result<Self, ConsignmentContractError> {
        let policy = Self {
            payment_deadline_days,
            reminder_limit: REMINDER_LIMIT,
            cod_return_limit,
            isekai_refusal_limit,
            isekai_refusal_window_days: ISEKAI_REFUSAL_WINDOW_DAYS,
        };
        policy.validate()?;
        Ok(policy)
    }

    /// 校验全部字段；字段公开，所以持久化读回的值也必须经此复核。
    pub fn validate(&self) -> Result<(), ConsignmentContractError> {
        if !Self::PAYMENT_DEADLINE_DAYS.contains(&self.payment_deadline_days) {
            return Err(ConsignmentContractError::Policy("付款期限天数越界"));
        }
        if !Self::COD_RETURN_LIMIT.contains(&self.cod_return_limit) {
            return Err(ConsignmentContractError::Policy("COD 退回上限越界"));
        }
        if !Self::ISEKAI_REFUSAL_LIMIT.contains(&self.isekai_refusal_limit) {
            return Err(ConsignmentContractError::Policy("异世界拒付上限越界"));
        }
        if self.reminder_limit != REMINDER_LIMIT
            || self.isekai_refusal_window_days != ISEKAI_REFUSAL_WINDOW_DAYS
        {
            return Err(ConsignmentContractError::Policy("固定常量被改写"));
        }
        Ok(())
    }
}

/// 资金账户的记账方式，对应 `settlement_funding_accounts.accounting_mode`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingAccountingMode {
    StrictCash,
    RecordOnly,
}

/// 买前核过的原 Bot 资金账户冻结（R5-C9）：账户、记账方式与“原可认证开户边界”，
/// 边界 = 购买时最近一次 opening 的来源与账户版本，供 certify_cod 复验原 Send 严格晚于边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleFundingFreeze {
    pub uid: String,
    pub mode: FundingAccountingMode,
    pub boundary_at: Timestamp,
    /// 最近一次 opening 的来源；record_only 无现金 opening 时为空。
    pub boundary_source_id: Option<Uuid>,
    /// 购买时的账户版本，非负。
    pub account_version: i64,
}

/// 构造 [`SaleTerms`] 的输入；报价与定价版本由构造器重算，不接受外部给定。
#[derive(Debug, Clone)]
pub struct SaleTermsInput {
    pub buyer_user_id: UserId,
    pub frozen_buyer_identity: String,
    pub listing_version: ListingVersion,
    pub price: ListingPrice,
    pub fee_bps: FeeBps,
    pub payment_deadline_at: Timestamp,
    pub policy: ConsignmentSalePolicy,
    pub funding: SaleFundingFreeze,
}

/// Sale 合同条款：成交时冻结的买家、金额、策略与资金账户。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaleTerms {
    pub buyer_user_id: UserId,
    pub frozen_buyer_identity: String,
    pub listing_version: ListingVersion,
    pub price: ListingPrice,
    pub fee_bps: FeeBps,
    pub quote: ListingQuote,
    pub pricing_version: u16,
    pub payment_deadline_at: Timestamp,
    pub policy: ConsignmentSalePolicy,
    pub funding: SaleFundingFreeze,
}

impl SaleTerms {
    /// 按 `quote_v1(price, fee_bps)` 重算 F 与 S 后构造并校验。
    pub fn new(input: SaleTermsInput) -> Result<Self, ConsignmentContractError> {
        let terms = Self {
            buyer_user_id: input.buyer_user_id,
            frozen_buyer_identity: input.frozen_buyer_identity,
            listing_version: input.listing_version,
            price: input.price,
            fee_bps: input.fee_bps,
            quote: quote_v1(input.price, input.fee_bps),
            pricing_version: PRICING_VERSION,
            payment_deadline_at: input.payment_deadline_at,
            policy: input.policy,
            funding: input.funding,
        };
        terms.validate()?;
        Ok(terms)
    }

    /// 金额守恒（不变量 6）：F = floor(P·bps/10⁴)、S = P − F，且 F + S = P；
    /// 持久化读回的字段与重算结果不一致即拒绝，不静默修正。
    pub fn validate(&self) -> Result<(), ConsignmentContractError> {
        if self.buyer_user_id.is_nil() {
            return Err(ConsignmentContractError::Sale("买家标识为零"));
        }
        if !valid_text(&self.frozen_buyer_identity, MAX_FROZEN_TEXT_BYTES) {
            return Err(ConsignmentContractError::Sale("买家冻结身份无效"));
        }
        if self.pricing_version != PRICING_VERSION {
            return Err(ConsignmentContractError::Sale("定价版本不受支持"));
        }
        let expected = quote_v1(self.price, self.fee_bps);
        if self.quote != expected
            || self.quote.fee.checked_add(self.quote.seller_amount) != Some(self.price.value())
            || self.quote.seller_amount <= 0
        {
            return Err(ConsignmentContractError::Sale("手续费与卖家所得和标价不符"));
        }
        if !valid_text(&self.funding.uid, MAX_FROZEN_TEXT_BYTES) {
            return Err(ConsignmentContractError::Sale("资金账户标识无效"));
        }
        if self.funding.account_version < 0 {
            return Err(ConsignmentContractError::Sale("资金账户版本无效"));
        }
        self.policy.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::ConsignmentSalePolicy;

    #[test]
    fn policy_accepts_range_edges_and_rejects_outside() {
        assert!(ConsignmentSalePolicy::new(1, 2, 2).is_ok());
        assert!(ConsignmentSalePolicy::new(90, 10, 100).is_ok());
        for (days, cod, refusal) in
            [(0, 2, 2), (91, 2, 2), (1, 1, 2), (1, 11, 2), (1, 2, 1), (1, 2, 101)]
        {
            assert!(
                ConsignmentSalePolicy::new(days, cod, refusal).is_err(),
                "{days}/{cod}/{refusal}"
            );
        }
    }
}
