//! 装备 header 的等级与交易状态协议；大类名称复用领域词典。

use auction_application::core::diagnostics::DiagnosticContext;
use auction_domain::equipment::taxonomy::EquipmentCategory;

use super::{EquipmentParseError, identity::taxonomy_error};
use crate::safe_diagnostics;

/// 保留来源等级状态；旧 DTO 将非数字状态投影为 None，不把无等级伪装成零级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EquipmentLevel {
    Unspecified,
    Unassigned,
    Assigned(i32),
    /// 三个 Tier 数值在读取时已校验；现有 DTO 不持久化这三个数值。
    SoulboundTier,
}

impl EquipmentLevel {
    pub(crate) fn from_legacy(level: Option<i32>) -> Self {
        level.map_or(Self::Unspecified, Self::Assigned)
    }

    pub(crate) fn legacy_level(self) -> Option<i32> {
        match self {
            Self::Assigned(level) => Some(level),
            Self::Unspecified | Self::Unassigned | Self::SoulboundTier => None,
        }
    }
}

pub(super) struct EquipmentHeader {
    pub(super) category: EquipmentCategory,
    pub(super) level: EquipmentLevel,
    pub(super) trade_status: EquipmentTradeStatus,
}

/// 来源交易状态与应用准入决定分开；未声明状态仍沿用旧 DTO 的 false 投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EquipmentTradeStatus {
    Unspecified,
    Tradeable,
    Soulbound,
}

impl EquipmentTradeStatus {
    pub(super) fn is_tradeable(&self) -> bool {
        matches!(self, Self::Tradeable)
    }
}

pub(super) fn parse_header(value: &str) -> Result<EquipmentHeader, EquipmentParseError> {
    let mut fields: Vec<&str> = value.split_whitespace().collect();
    let trade_status = match fields.last().copied() {
        Some("Tradeable") => EquipmentTradeStatus::Tradeable,
        Some("Soulbound") => EquipmentTradeStatus::Soulbound,
        _ => EquipmentTradeStatus::Unspecified,
    };
    if !matches!(trade_status, EquipmentTradeStatus::Unspecified) {
        fields.pop();
    }
    let mut level = EquipmentLevel::Unspecified;
    if let Some(index) = fields.iter().position(|field| *field == "Tier") {
        // Tier 仅允许真实 Soulbound 页面，三个阶段值仍逐项受 0..=1000 限制。
        if !matches!(trade_status, EquipmentTradeStatus::Soulbound) || index + 1 >= fields.len() {
            return Err(EquipmentParseError::InvalidLevel);
        }
        let tiers = fields[index + 1..].join("");
        let tiers: Vec<&str> = tiers.split('/').collect();
        if tiers.len() != 3
            || tiers.iter().any(|tier| match tier.parse::<i32>() {
                Ok(tier) => !(0..=1_000).contains(&tier),
                Err(diagnostic_cause) => {
                    safe_diagnostics::warn(
                        "equipment_fetch",
                        "parse_forge_tier",
                        safe_diagnostics::project(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    true
                }
            })
        {
            return Err(EquipmentParseError::InvalidLevel);
        }
        level = EquipmentLevel::SoulboundTier;
        fields.truncate(index);
    } else if let Some(index) = fields.iter().position(|field| *field == "Level") {
        let raw = fields.get(index + 1).ok_or(EquipmentParseError::InvalidLevel)?;
        level = if *raw == "Unassigned" {
            EquipmentLevel::Unassigned
        } else {
            let parsed = raw.parse::<i32>().map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "equipment_fetch",
                    "parse_level",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                EquipmentParseError::InvalidLevel
            })?;
            if !(1..=100_000).contains(&parsed) {
                return Err(EquipmentParseError::InvalidLevel);
            }
            EquipmentLevel::Assigned(parsed)
        };
        fields.drain(index..=index + 1);
    }
    let category =
        EquipmentCategory::from_header_label(&fields.join(" ")).map_err(taxonomy_error)?;
    Ok(EquipmentHeader { category, level, trade_status })
}
