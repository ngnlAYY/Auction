//! 寄售资金的纯领域规则：COD 实收跨 opening 拆分、成交分账、手续费分录、退款转换与现金退件的 UID 级 opening 阻断。
//!
//! 规则来源见 D9 第 4.2–4.4 节与决定 19：手续费单独记账，不进补偿池，退款只冲回本笔手续费。
//! 本模块不读时钟、不访问存储；存储车道只落实这里给出的金额与判定。

mod blocker;
mod distribution;
mod errors;
mod fee;
mod principal;
mod receipt_split;
mod refund;

pub use blocker::{
    BlockClearanceEvidence, BlockedLeg, OpeningBlock, check_opening_gate, may_clear_block,
};
pub use distribution::{
    FundingMode, SaleDistribution, TransitOutput, TransitPlan, expected_gross, plan_cod_funding,
    sale_distribution,
};
pub use errors::FundingRuleError;
pub use fee::{FeeEarned, FeeReversal, FeeRows, fee_rows, reverse_fee};
pub use principal::{PrincipalConclusion, resolve_principal_conclusion};
pub use receipt_split::{CodReceiptSplit, OpeningSlice, opening_included_total};
pub use refund::{RefundConversion, buyer_refund_liability, refund_conversion};
