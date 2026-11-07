//! W2.5 处置读侧的运行时口径与影子比对。
//!
//! 读侧只有两种事实来源：迁移前的专用排除谓词（`legacy`）与统一处置事实
//! `delivery_dispositions.resolves_alert`（`classified`）。本模块只做纯选择与逐项比较，
//! 不含 SQL、不读取配置，也不改变任何告警阈值、投递、资金或履约结果。
//!
//! 三种模式的含义固定为（按计划 §1.5 的切换顺序）：
//! - `legacy`：只用迁移前按来源拼接的排除谓词，等价于 W2 之前的读侧；
//! - `shadow`：**可见值仍是旧口径**（`legacy` 权威），另外逐项比对分类口径，差异只进
//!   结构化日志与影子指标，绝不改变告警可见值；这是本版缺省模式；
//! - `classified`：直接用统一处置事实的 `resolves_alert` 过滤“未处置”，只有在影子期
//!   差异经过观察与批准后由操作者显式切换。
//!
//! 影子样本的分支标签就是 [`DispositionReadField`] 的闭集标签，因此每个被比较的字段
//! 都必须在 [`crate::ports::shadow_run::ShadowOwner::DispositionReads`] 登记；未登记的分支
//! 不会被记录，也就不能作为“差异为零”的证据。

use serde::{Deserialize, Serialize};

use crate::ports::shadow_run::ShadowOwner;

/// 读侧口径；数据库开关的合法值就是三个稳定标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispositionReadMode {
    /// 迁移前的专用排除谓词，只用于回退验证。
    Legacy,
    /// 保留 `classified` 结果，只记录与 `legacy` 的逐项差异。
    Shadow,
    /// 统一处置事实 `resolves_alert`，当前生产口径。
    Classified,
}

impl DispositionReadMode {
    /// 本版缺省口径：旧口径继续持有裁决权，只比较分类口径（计划 §1.5）。
    pub const DEFAULT: Self = Self::Shadow;

    /// 返回数据库开关与日志使用的稳定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Shadow => "shadow",
            Self::Classified => "classified",
        }
    }

    /// 解析开关取值；未知取值不是损坏数据，调用方保留缺省口径。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "legacy" => Some(Self::Legacy),
            "shadow" => Some(Self::Shadow),
            "classified" => Some(Self::Classified),
            _ => None,
        }
    }
}

impl Default for DispositionReadMode {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 被比较的告警读数字段闭集；标签同时是影子指标的分支标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispositionReadField {
    /// 投递来源的失败计数。
    DeliveryFailed,
    /// 非结算通知来源的失败计数。
    NotificationFailed,
    /// 通知失败的最老年龄。
    NotificationOldestFailedAge,
    /// 通知来源的邮资不足计数。
    NotificationPostage,
    /// 邮资不足的最老年龄。
    NotificationOldestPostageAge,
    /// 履约来源的失败计数（装备损失后的线下补偿）。
    FulfillmentFailed,
}

impl DispositionReadField {
    /// 全部被比较字段；顺序固定，便于逐项断言。
    pub const ALL: [Self; 6] = [
        Self::DeliveryFailed,
        Self::NotificationFailed,
        Self::NotificationOldestFailedAge,
        Self::NotificationPostage,
        Self::NotificationOldestPostageAge,
        Self::FulfillmentFailed,
    ];

    /// 返回影子指标分支与日志使用的稳定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::DeliveryFailed => "delivery_failed",
            Self::NotificationFailed => "notification_failed",
            Self::NotificationOldestFailedAge => "notification_oldest_failed_age",
            Self::NotificationPostage => "notification_postage",
            Self::NotificationOldestPostageAge => "notification_oldest_postage_age",
            Self::FulfillmentFailed => "fulfillment_failed",
        }
    }
}

/// 两种口径使用同一结构承载“未处置失败”读数，逐项比较不需要额外映射表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispositionReadCounts {
    pub delivery_failed: i64,
    pub notification_failed: i64,
    pub notification_oldest_failed_age_seconds: i64,
    pub notification_postage: i64,
    pub notification_oldest_postage_age_seconds: i64,
    pub fulfillment_failed: i64,
}

impl DispositionReadCounts {
    /// 按闭集字段取值；影子比对与断言共用同一入口。
    pub const fn get(self, field: DispositionReadField) -> i64 {
        match field {
            DispositionReadField::DeliveryFailed => self.delivery_failed,
            DispositionReadField::NotificationFailed => self.notification_failed,
            DispositionReadField::NotificationOldestFailedAge => {
                self.notification_oldest_failed_age_seconds
            }
            DispositionReadField::NotificationPostage => self.notification_postage,
            DispositionReadField::NotificationOldestPostageAge => {
                self.notification_oldest_postage_age_seconds
            }
            DispositionReadField::FulfillmentFailed => self.fulfillment_failed,
        }
    }

    /// 全部不一致字段；空向量表示两个口径逐项一致。
    pub fn differences(self, other: Self) -> Vec<DispositionReadField> {
        DispositionReadField::ALL
            .into_iter()
            .filter(|field| self.get(*field) != other.get(*field))
            .collect()
    }
}

/// 一次选择的最终读数与（仅影子模式的）差异字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionReadSelection {
    /// 本次实际对告警可见的读数。
    pub counts: DispositionReadCounts,
    /// 只在影子模式非空；顺序与 [`DispositionReadField::ALL`] 一致。
    pub mismatches: Vec<DispositionReadField>,
}

/// 按模式选择读数；纯函数，调用方负责提供两个口径的真实读数。
///
/// 影子模式返回 `legacy`：旧口径持有裁决权，分类口径只产生待观察的差异样本。把分类口径
/// 提前变成可见值会绕过计划 §1.5 的观察与批准顺序。
pub fn select_read_counts(
    mode: DispositionReadMode,
    classified: DispositionReadCounts,
    legacy: DispositionReadCounts,
) -> DispositionReadSelection {
    match mode {
        DispositionReadMode::Legacy => {
            DispositionReadSelection { counts: legacy, mismatches: Vec::new() }
        }
        DispositionReadMode::Classified => {
            DispositionReadSelection { counts: classified, mismatches: Vec::new() }
        }
        DispositionReadMode::Shadow => {
            DispositionReadSelection { mismatches: classified.differences(legacy), counts: legacy }
        }
    }
}

/// 影子 owner；调用方用它记录比对样本，避免出现第二套 label 约定。
pub const SHADOW_OWNER: ShadowOwner = ShadowOwner::DispositionReads;

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::ports::operations::alert::disposition_read as parent;

    fn counts(
        delivery_failed: i64,
        notification_failed: i64,
        oldest_failed: i64,
        postage: i64,
        oldest_postage: i64,
    ) -> DispositionReadCounts {
        DispositionReadCounts {
            delivery_failed,
            notification_failed,
            notification_oldest_failed_age_seconds: oldest_failed,
            notification_postage: postage,
            notification_oldest_postage_age_seconds: oldest_postage,
            fulfillment_failed: 0,
        }
    }

    #[test]
    fn mode_labels_round_trip_and_reject_unknown_values() {
        for mode in [
            DispositionReadMode::Legacy,
            DispositionReadMode::Shadow,
            DispositionReadMode::Classified,
        ] {
            assert_eq!(DispositionReadMode::parse(mode.label()), Some(mode));
        }
        assert_eq!(DispositionReadMode::parse("permanent"), None);
        assert_eq!(DispositionReadMode::parse(""), None);
        assert_eq!(DispositionReadMode::default(), DispositionReadMode::Shadow);
    }

    #[test]
    fn legacy_mode_returns_the_legacy_counts_without_mismatch_noise() {
        let selection = select_read_counts(
            DispositionReadMode::Legacy,
            counts(1, 2, 3, 4, 5),
            counts(9, 8, 7, 6, 5),
        );
        assert_eq!(selection.counts, counts(9, 8, 7, 6, 5));
        assert!(selection.mismatches.is_empty());
    }

    #[test]
    fn classified_mode_returns_the_classified_counts() {
        let classified = counts(1, 2, 3, 4, 5);
        let selection =
            select_read_counts(DispositionReadMode::Classified, classified, counts(9, 8, 7, 6, 5));
        assert_eq!(selection.counts, classified);
        assert!(selection.mismatches.is_empty());
    }

    #[test]
    fn shadow_mode_keeps_legacy_results_and_reports_every_difference() {
        let classified = counts(1, 2, 3, 4, 5);
        let legacy = counts(9, 2, 3, 6, 5);
        let selection = select_read_counts(DispositionReadMode::Shadow, classified, legacy);
        assert_eq!(selection.counts, legacy, "影子模式必须继续显示旧口径的裁决值");
        assert_eq!(
            selection.mismatches,
            vec![DispositionReadField::DeliveryFailed, DispositionReadField::NotificationPostage]
        );

        let matching = select_read_counts(DispositionReadMode::Shadow, classified, classified);
        assert_eq!(matching.counts, classified);
        assert!(matching.mismatches.is_empty());
    }

    #[test]
    fn fulfillment_terminal_is_compared_like_the_other_fields() {
        let mut classified = counts(1, 2, 3, 4, 5);
        classified.fulfillment_failed = 0;
        let mut legacy = counts(1, 2, 3, 4, 5);
        legacy.fulfillment_failed = 1;
        assert_eq!(classified.differences(legacy), vec![DispositionReadField::FulfillmentFailed]);
        let selection = select_read_counts(DispositionReadMode::Shadow, classified, legacy);
        assert_eq!(selection.counts, legacy);
    }

    #[test]
    fn field_labels_are_registered_shadow_branches() {
        let branches = SHADOW_OWNER.branches();
        for field in DispositionReadField::ALL {
            assert!(branches.contains(&field.label()), "影子分支缺少字段标签 {}", field.label());
        }
        assert_eq!(branches.len(), DispositionReadField::ALL.len(), "分支闭集不能有未比较的项");
    }
}
