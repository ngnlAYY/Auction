//! 购买成交的 Sale 条款构造：付款期限 D0 只由锁内成交时刻与冻结策略派生。

use auction_time::{Duration, Timestamp};

use super::decision::PurchaseGrant;
use crate::UserId;
use crate::consignment::{
    ConsignmentContractError, ConsignmentSalePolicy, ListingVersion, SaleTerms, SaleTermsInput,
};

/// D0 = 成交时刻 + 冻结的 `payment_deadline_days`（按 24 小时一天）。
///
/// 保护不变量 10：期限只由冻结策略与锁内时刻决定，不接受外部传入，也不随配置后续变化。
pub fn payment_deadline_at(
    committed_at: Timestamp,
    policy: &ConsignmentSalePolicy,
) -> Result<Timestamp, ConsignmentContractError> {
    policy.validate()?;
    committed_at
        .checked_add_signed(Duration::days(i64::from(policy.payment_deadline_days)))
        .ok_or(ConsignmentContractError::Sale("付款期限无法表示"))
}

/// 复核已构造的条款：`payment_deadline_at` 必须恰好等于成交时刻加冻结天数。
pub fn verify_payment_deadline(
    committed_at: Timestamp,
    terms: &SaleTerms,
) -> Result<(), ConsignmentContractError> {
    if payment_deadline_at(committed_at, &terms.policy)? != terms.payment_deadline_at {
        return Err(ConsignmentContractError::Sale("付款期限与成交时刻不符"));
    }
    Ok(())
}

/// 购买成交构造 Sale 条款的输入；没有付款期限字段。
#[derive(Debug, Clone)]
pub struct PurchaseSaleInput {
    /// 锁后数据库时刻，即成交时刻。
    pub committed_at: Timestamp,
    pub buyer_user_id: UserId,
    pub frozen_buyer_identity: String,
    /// 成交所依据的挂单版本（命令的预期版本，已与锁内事实相等）。
    pub listing_version: ListingVersion,
    pub grant: PurchaseGrant,
    pub policy: ConsignmentSalePolicy,
}

/// 由授予的价格与费率构造 Sale 条款；D0 在此派生，金额由 `SaleTerms::new` 重算并与授予逐项比对。
pub fn build_purchase_sale_terms(
    input: PurchaseSaleInput,
) -> Result<SaleTerms, ConsignmentContractError> {
    let payment_deadline_at = payment_deadline_at(input.committed_at, &input.policy)?;
    let terms = SaleTerms::new(SaleTermsInput {
        buyer_user_id: input.buyer_user_id,
        frozen_buyer_identity: input.frozen_buyer_identity,
        listing_version: input.listing_version,
        price: input.grant.price,
        fee_bps: input.grant.fee_bps,
        payment_deadline_at,
        policy: input.policy,
        funding: input.grant.funding.clone(),
    })?;
    if terms.quote != input.grant.quote || terms.pricing_version != input.grant.pricing_version {
        return Err(ConsignmentContractError::Sale("成交金额与授予不符"));
    }
    Ok(terms)
}
