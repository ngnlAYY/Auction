//! 生命周期继承与结算读取共用的冻结结算策略解码；不读取数据库或修改历史策略。

use auction_domain::{AdjustmentAnchor, SettlementMode, SettlementPolicy};
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
pub(super) fn decode_mode(mode: &str) -> Result<SettlementMode, &'static str> {
    SettlementMode::from_name(mode).ok_or("结算策略模式未知")
}

pub(super) fn decode(snapshot: &Value) -> Result<SettlementPolicy, &'static str> {
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
