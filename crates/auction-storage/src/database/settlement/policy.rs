//! 生命周期继承与结算读取共用的冻结结算策略解码，以及恢复记录首寄付款期限的换算；不读取
//! 数据库或修改历史策略。

use auction_domain::{AdjustmentAnchor, RecoveryPolicy, SettlementMode, SettlementPolicy};
use auction_time::Timestamp;
use serde::Deserialize;
use serde_json::Value;

/// 数据库快照沿用小写模式名，领域序列化形状不承担此兼容映射。
#[derive(Deserialize)]
struct StoredSettlementPolicy {
    version: u32,
    mode: String,
    compensation_pool_enabled: bool,
    isekai_carrier_credit: u32,
    anchors: Vec<AdjustmentAnchor>,
}

/// 冻结快照、履约和收款证明共用的模式闭集，不把未知标签降级为历史算法。
pub(in crate::database) fn decode_mode(mode: &str) -> Result<SettlementMode, &'static str> {
    SettlementMode::from_name(mode).ok_or("结算策略模式未知")
}

pub(in crate::database) fn decode(snapshot: &Value) -> Result<SettlementPolicy, &'static str> {
    let stored = StoredSettlementPolicy::deserialize(
        snapshot.get("settlement").ok_or("拍卖策略快照缺少 settlement")?,
    )
    .map_err(|_| "结算策略快照字段无效")?;
    let mode = decode_mode(&stored.mode)?;
    let policy = SettlementPolicy {
        version: stored.version,
        mode,
        compensation_pool_enabled: stored.compensation_pool_enabled,
        isekai_carrier_credit: stored.isekai_carrier_credit,
        anchors: stored.anchors,
    };
    // 这里只验证历史结构。新建后继时才进一步应用新模板金额门禁。
    policy.validate().map_err(|_| "结算策略快照校验失败")?;
    Ok(policy)
}

/// 恢复记录行到首寄付款期限 D0 的唯一换算：履约领取、COD 实收准入与载体资金门都按它重建付款
/// 邮件正文里的期限，不读重寄会改写的 `payment_deadline_at`。两列都缺失表示义务没有恢复记录
/// （历史义务）；只缺一列、版本越界或时间溢出都失败关闭，版本表由领域持有。
pub(in crate::database) fn initial_payment_deadline(
    created_at: Option<Timestamp>,
    policy_version: Option<i32>,
) -> Result<Option<Timestamp>, &'static str> {
    match (created_at, policy_version) {
        (None, None) => Ok(None),
        (Some(created_at), Some(version)) => u32::try_from(version)
            .ok()
            .and_then(|version| RecoveryPolicy::initial_payment_deadline(created_at, version))
            .map(Some)
            .ok_or("恢复记录的首寄付款期限无法按冻结策略换算"),
        _ => Err("恢复记录的首寄付款期限事实不完整"),
    }
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::initial_payment_deadline;

    #[test]
    fn initial_payment_deadline_follows_the_frozen_recovery_row() {
        let created_at =
            Timestamp::from_ymd_hms(2026, 11, 3, 12, 19, 28).expect("固定测试时间必须存在");
        assert_eq!(initial_payment_deadline(None, None), Ok(None));
        assert_eq!(
            initial_payment_deadline(Some(created_at), Some(1)),
            Ok(Some(created_at + Duration::days(7)))
        );
        assert_eq!(
            initial_payment_deadline(Some(created_at), Some(2)),
            Ok(Some(created_at + Duration::hours(24)))
        );
        // 缺列、负数或未知版本与时间溢出都不能退化为“没有期限”的旧载荷。
        for (created_at, version) in [
            (Some(created_at), None),
            (None, Some(1)),
            (Some(created_at), Some(-1)),
            (Some(created_at), Some(3)),
            (Some(Timestamp::MAX_UTC), Some(1)),
        ] {
            assert!(initial_payment_deadline(created_at, version).is_err(), "{version:?}");
        }
    }
}
