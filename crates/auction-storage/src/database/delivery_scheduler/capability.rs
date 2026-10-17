//! 当前构建的现金发送门能力；不证明线上版本，也不绕过真实资金/证据授权。

impl auction_application::FundingCashGateProvider for crate::DatabaseStore {
    fn funding_cash_gate(&self) -> Option<auction_application::FundingCashGate> {
        #[cfg(feature = "postgres")]
        {
            Some(auction_application::FundingCashGate::V1)
        }
        #[cfg(not(feature = "postgres"))]
        {
            None
        }
    }
}
