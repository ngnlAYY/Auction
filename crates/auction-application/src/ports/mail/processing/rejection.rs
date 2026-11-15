//! 装备投稿整封退回时写入拒收说明的原因闭集。

use super::closed_names::closed_names;

closed_names! {
    /// 装备投稿被整封退回的原因闭集。
    ///
    /// 过滤器、起拍价解析与入站处理器只产出本枚举，入队时经 [`Self::as_str`] 写成
    /// `auction_equipment_rejected` 说明信的 `result_code` 与入站决定的 `reason_code`；通知模板表
    /// 的结果码闭集由 [`Self::ALL`] 派生，发送端按本枚举穷举渲染说明文案，原因与持久化值只在此
    /// 声明一处。持久化值逐字不可改写；只追加不删除，已入队的历史结果码仍需被发送前解码与资产
    /// 冲突排除识别。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum EquipmentRejectionReason {
        /// 装备来源明确无效、读回身份不符或快照未通过校验；与说明信模板同名的通用原因。
        Unverified => "auction_equipment_rejected",
        /// 起拍价指令无法解析，或默认价、附件数量、附件 ID 不合法。
        StartPriceInvalid => "start_price_invalid",
        /// 已不再产生，保留给历史投递。
        StartPriceMissing => "start_price_missing",
        /// 同一附件出现多条起拍价指令。
        StartPriceDuplicate => "start_price_duplicate",
        /// 指令中的装备 key 与附件不符。
        StartPriceIdentityMismatch => "start_price_identity_mismatch",
        /// 指令引用了未随信附带的装备。
        StartPriceAttachmentNotFound => "start_price_attachment_not_found",
        /// 品质不满足硬性门槛（含早鸟窗口的品质列表）。
        QualityNotAllowed => "quality_not_allowed",
        PrefixNotAllowed => "prefix_not_allowed",
        SuffixNotAllowed => "suffix_not_allowed",
        CategoryNotAllowed => "category_not_allowed",
        EquipmentTypeNotAllowed => "equipment_type_not_allowed",
        /// 等级缺失或超出配置的上下限。
        LevelNotAllowed => "level_not_allowed",
        /// 外部装备 ID 已登记且不满足复用条件。
        EquipmentConflict => "auction_equipment_conflict",
        /// 装备属性不是过滤器能识别的结构。
        AttributesInvalid => "attributes_invalid",
        /// 收件目标在新赛季开始前拒收装备。
        SeasonIntakeClosed => "season_intake_closed",
        /// 装备不可交易。
        TradeableRequired => "tradeable_required",
    }
}

impl EquipmentRejectionReason {
    /// 解析载荷中冻结的结果码；闭集外的值返回 `None`，由调用方失败关闭。
    pub fn from_code(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::EquipmentRejectionReason;

    /// 持久化值逐字固定：线上已有 `auction_equipment_conflict`、`auction_equipment_rejected`、
    /// `start_price_attachment_not_found` 与 `start_price_invalid` 的历史行，改名会让它们无法识别。
    #[test]
    fn persisted_codes_are_frozen_in_order() {
        assert_eq!(
            EquipmentRejectionReason::ALL.map(EquipmentRejectionReason::as_str),
            [
                "auction_equipment_rejected",
                "start_price_invalid",
                "start_price_missing",
                "start_price_duplicate",
                "start_price_identity_mismatch",
                "start_price_attachment_not_found",
                "quality_not_allowed",
                "prefix_not_allowed",
                "suffix_not_allowed",
                "category_not_allowed",
                "equipment_type_not_allowed",
                "level_not_allowed",
                "auction_equipment_conflict",
                "attributes_invalid",
                "season_intake_closed",
                "tradeable_required",
            ]
        );
    }

    #[test]
    fn codes_round_trip_and_stay_within_the_text_bound() {
        let codes = EquipmentRejectionReason::ALL
            .into_iter()
            .map(EquipmentRejectionReason::as_str)
            .collect::<HashSet<_>>();
        assert_eq!(codes.len(), EquipmentRejectionReason::ALL.len(), "结果码不得重复");
        for reason in EquipmentRejectionReason::ALL {
            assert_eq!(EquipmentRejectionReason::from_code(reason.as_str()), Some(reason));
            assert!(auction_domain::valid_text(reason.as_str(), 256), "{reason:?}");
        }
        for unknown in ["", "unknown_reason", "Quality_Not_Allowed", "quality_not_allowed "] {
            assert_eq!(EquipmentRejectionReason::from_code(unknown), None, "{unknown}");
        }
    }
}
