//! 买家确领原件、载体 COD 付款与现金确领三类证明的验证器签收口径（D9 §2.3 `recipient_took_original`、
//! `remote_cod_paid`、`remote_cash_claimed`）。
//!
//! 保护不变量 4、6：观察不会自动成为结论，只有已签收的验证器版本产出的证据才能推进确领（原件交接与卖款放行、
//! 现金 `claimed`）与收款认证。D9 §2.3 要求原件、载体 COD 与无 COD 现金各自签收版本化验证器；本期三类都未签收
//! （R5-C10，成对样本未到），生产路径恒不推进、保留原占用与受益人负债，签收是单独提交。main 原件 COD 的付款与
//! 确领是同一次领取，共用原件类签收。

use auction_domain::consignment::dispatch::EvidenceCategory;

/// 已签收的原件确领验证器版本（含 main 原件 COD 的付款）；本期为空。
const SIGNED_ORIGINAL_CLAIM_VALIDATORS: &[&str] = &[];

/// 已签收的载体 COD 付款验证器版本；本期为空。
const SIGNED_CARRIER_PAYMENT_VALIDATORS: &[&str] = &[];

/// 已签收的现金确领验证器版本；本期为空。
const SIGNED_CASH_CLAIM_VALIDATORS: &[&str] = &[];

/// 仅 `test-db` 构建认的已签收验证器版本（与退件消费的测试版本同名），供夹具走通正例；生产构建不含它。
#[cfg(feature = "test-db")]
const TEST_SIGNED_OFF_VALIDATOR: &str = "test-signed-off-validator-v1";

/// 原件确领证据的验证器是否已签收。
pub(in crate::database) fn original_claim_signed(version: &str) -> bool {
    signed(SIGNED_ORIGINAL_CLAIM_VALIDATORS, version)
}

/// 现金确领证据的验证器是否已签收。
pub(in crate::database) fn cash_claim_signed(version: &str) -> bool {
    signed(SIGNED_CASH_CLAIM_VALIDATORS, version)
}

/// 按证据类别判签收：原件确领、载体付款、现金确领各用自己的清单；Sent 绑定与 RTS 不是领取或付款证明，恒为否。
pub(in crate::database) fn category_signed(category: EvidenceCategory, version: &str) -> bool {
    match category {
        EvidenceCategory::OriginalClaim => original_claim_signed(version),
        EvidenceCategory::CarrierPayment => signed(SIGNED_CARRIER_PAYMENT_VALIDATORS, version),
        EvidenceCategory::CashClaim => cash_claim_signed(version),
        EvidenceCategory::SentBinding | EvidenceCategory::Rts => false,
    }
}

/// 该类别已签收的全部版本（`test-db` 构建另含测试版本），供待办查询在 SQL 里按签收过滤。
pub(in crate::database) fn signed_versions(category: EvidenceCategory) -> Vec<&'static str> {
    let list: &[&'static str] = match category {
        EvidenceCategory::OriginalClaim => SIGNED_ORIGINAL_CLAIM_VALIDATORS,
        EvidenceCategory::CarrierPayment => SIGNED_CARRIER_PAYMENT_VALIDATORS,
        EvidenceCategory::CashClaim => SIGNED_CASH_CLAIM_VALIDATORS,
        EvidenceCategory::SentBinding | EvidenceCategory::Rts => &[],
    };
    #[cfg(feature = "test-db")]
    if !matches!(category, EvidenceCategory::SentBinding | EvidenceCategory::Rts) {
        return list.iter().copied().chain([TEST_SIGNED_OFF_VALIDATOR]).collect();
    }
    list.to_vec()
}

fn signed(list: &[&str], version: &str) -> bool {
    #[cfg(feature = "test-db")]
    if version == TEST_SIGNED_OFF_VALIDATOR {
        return true;
    }
    list.contains(&version)
}
