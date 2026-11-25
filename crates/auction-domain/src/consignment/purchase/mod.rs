//! 寄售购买与 v2 改价的领域判定：锁内事实快照、拒绝优先序、金额授予与付款期限 D0 派生。
//!
//! 纯规则，不读时钟、不访问存储；锁、事务与写入由存储车道承担。

mod decision;
mod facts;
mod rejection;
mod reprice;
mod sale;

pub use decision::{PurchaseGrant, decide_purchase};
pub use facts::{
    PurchaseFacts, PurchaseFunding, PurchaseInput, PurchaseListing, PurchaseParty, PurchasePosition,
};
pub use rejection::PurchaseRejection;
pub use reprice::{
    RepriceDecision, RepriceFacts, RepriceInput, RepriceRejection, decide_reprice_available,
};
pub use sale::{
    PurchaseSaleInput, build_purchase_sale_terms, payment_deadline_at, verify_payment_deadline,
};

#[cfg(test)]
mod tests;
