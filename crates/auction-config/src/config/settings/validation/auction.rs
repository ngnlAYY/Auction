//! 拍卖经济、结算和恢复策略校验。

use super::super::*;

pub(crate) fn validate_auction(config: &AuctionConfig) -> Result<(), ConfigError> {
    if !matches!(config.settlement.mode.as_str(), "legacy_zero_v0" | "hold_curve_v1") {
        return Err(ConfigError::InvalidAuctionConfig(
            "settlement.mode 只能是 legacy_zero_v0 或 hold_curve_v1",
        ));
    }
    if config.settlement.recovery.mode != "remind_v1" {
        return Err(ConfigError::InvalidAuctionConfig("settlement.recovery.mode 只能是 remind_v1"));
    }
    config
        .policy()
        .validate()
        .map_err(|_| ConfigError::InvalidAuctionConfig("经济、结算或恢复字段边界无效"))
}
