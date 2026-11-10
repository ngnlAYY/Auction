//! 资产对账与偿付能力报告的窄端口与稳定报告模型。
//!
//! 报告只读取数据库投影与只读外部观察：不领取工作项、不发送邮件、不转移余额或装备，
//! 也不授权任何资金/资产动作（`asset_actions_authorized` 恒为 false）。外部 Armory 观察
//! 采用与发送前库存证明相同的七类 organize 页面；未观察到的装备只表示“本轮未在 Armory
//! 看到”，不是装备丢失或可处置的结论。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use auction_domain::SettlementWorld;

use crate::ports::auction::ApplicationFuture;

/// 一次 Armory 只读观察；`complete` 为 false 时不参与差异结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetArmoryObservation {
    pub world: SettlementWorld,
    pub observed_at: DateTime<Utc>,
    /// 七个 organize 分类中观察到的外部装备 ID 并集；由观察方去重。
    pub equipment_ids: Vec<String>,
    /// 全部七类页面都通过结构校验时为 true；任一分类失败即 false。
    pub complete: bool,
    /// 固定失败码；不接受外部原始错误、URL 或 Cookie。
    pub failure_code: Option<&'static str>,
}

impl AssetArmoryObservation {
    /// 报告使用的稳定观察状态。
    pub const fn status(&self) -> &'static str {
        if self.complete { "observed" } else { "unavailable" }
    }

    /// 校验 ID 闭集与时间边界；不合法观察不得进入报告。
    pub fn validate(&self) -> bool {
        self.observed_at.timestamp_nanos_opt().is_some()
            && self.equipment_ids.len() <= 100_000
            && self
                .equipment_ids
                .iter()
                .all(|id| crate::ports::settlement::valid_external_positive_id(id))
            && self.equipment_ids.windows(2).all(|ids| ids[0] < ids[1])
            && (self.complete || self.failure_code.is_some())
            && self.failure_code.is_none_or(|code| code.len() <= 64)
    }
}

/// 单件应持装备的固定原因码；只暴露数据库业务标识。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyMissingSample {
    pub external_equipment_id: String,
    pub reason: String,
}

/// 单个 world 的装备持有核对结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyWorldReport {
    pub world: String,
    /// `observed` 或 `unavailable`；不可用时计数保持 0 且不产生差异结论。
    pub observation: String,
    pub expected_count: i64,
    pub observed_count: i64,
    pub missing_count: i64,
    /// 数据库已知、但不属于当前拍卖相关应持集合的普通库存。
    pub stock_count: i64,
    /// 数据库未知、已确认属于个人装备的观察项；不参与拍卖对账差异。
    pub personal_count: i64,
    /// 保留字段；当前拍卖对账不再把未知个人装备视为异常，恒为 0。
    pub unexpected_count: i64,
    pub missing_samples: Vec<CustodyMissingSample>,
    pub unexpected_samples: Vec<String>,
    pub samples_truncated: bool,
    pub failure_code: Option<String>,
}

/// 装备持有核对报告；追加保存到 `audit_records`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyReconciliationReport {
    pub format: String,
    pub report_id: Uuid,
    pub database_name: String,
    pub completed_at: DateTime<Utc>,
    pub sample_limit_per_world: i64,
    pub complete: bool,
    pub consistent: bool,
    pub worlds: Vec<CustodyWorldReport>,
    pub failure_code: Option<String>,
    pub asset_actions_authorized: bool,
}

/// 单个 world 的偿付能力结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolvencyWorldReport {
    pub world: String,
    pub pending_seller_payout_count: i64,
    pub pending_seller_payout_credits: i64,
    pub pending_refund_count: i64,
    pub pending_refund_credits: i64,
    pub pending_outbound_mail_count: i64,
    pub postage_estimate_credits: i64,
    pub required_credits: i64,
    pub balance_credits: Option<i64>,
    pub balance_observed_at: Option<DateTime<Utc>>,
    pub balance_status: String,
    pub evidence_status: String,
    /// `sufficient`、`shortfall` 或 `unknown`；余额缺失/过期时不得声称足够。
    pub verdict: String,
    pub shortfall_credits: i64,
}

/// 偿付能力报告；只报告不改账，个人消费与 Bot 共用账号时不做余额漂移归因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolvencyReport {
    pub format: String,
    pub report_id: Uuid,
    pub database_name: String,
    pub completed_at: DateTime<Utc>,
    /// 每条待发出站邮件的估算邮资；金额未核实前保持 0 并标记 `postage_rate_verified=false`。
    pub postage_rate_credits: i64,
    pub postage_rate_verified: bool,
    pub complete: bool,
    pub solvent: bool,
    pub worlds: Vec<SolvencyWorldReport>,
    pub failure_code: Option<String>,
    pub asset_actions_authorized: bool,
}

impl SolvencyReport {
    /// 所有 world 都有可判定余额且无缺口时为 solvent；未知余额不能当作足够。
    pub fn is_solvent(&self) -> bool {
        self.complete && self.worlds.iter().all(|world| world.verdict == "sufficient")
    }
}

/// 最近一份追加报告的安全摘要；仪表盘与告警只读取该投影。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AssetReportSummary {
    pub custody: Option<CustodyReconciliationReport>,
    pub solvency: Option<SolvencyReport>,
}

/// 数据库 owner：读取应持装备与负债投影，并把报告追加为审计事实。
pub trait AssetReconciliationStore: Send + Sync {
    fn reconcile_custody(
        &self,
        observations: Vec<AssetArmoryObservation>,
    ) -> ApplicationFuture<'_, CustodyReconciliationReport>;

    /// `balance_evidence_max_age_seconds` 必须来自当前有效的 recovery 配置；
    /// 报告不能用固定常量替代运行配置。
    fn report_solvency(
        &self,
        balance_evidence_max_age_seconds: u64,
    ) -> ApplicationFuture<'_, SolvencyReport>;

    /// 只读取最近追加的两类报告；缺失返回 None，不伪造空报告。
    fn latest_asset_reports(&self) -> ApplicationFuture<'_, AssetReportSummary>;
}

/// 只读 Armory 观察端口；实现必须使用既有只读凭据会话，不得执行资产动作。
pub trait AssetInventoryReader: Send + Sync {
    fn read_inventory(
        &self,
        world: SettlementWorld,
        observed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AssetArmoryObservation>;
}

/// 对账观察的 world 闭集；顺序参与报告字段顺序，必须稳定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetReconciliationConfig {
    pub worlds: Vec<SettlementWorld>,
    /// 余额证据在报告时的最大可信年龄；必须与 Worker recovery 的有效配置一致。
    pub balance_evidence_max_age_seconds: u64,
}

impl AssetReconciliationConfig {
    /// 只接受 1–2 个不重复且按名称递增的 world，避免同一轮重复观察或随机顺序。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.worlds.is_empty()
            || self.worlds.len() > 2
            || self
                .worlds
                .iter()
                .any(|world| !matches!(world, SettlementWorld::Main | SettlementWorld::Isekai))
            || self.worlds.windows(2).any(|pair| pair[0].as_str() >= pair[1].as_str())
        {
            return Err("资产对账 world 配置无效");
        }
        if self.balance_evidence_max_age_seconds == 0
            || self.balance_evidence_max_age_seconds > 86_400
        {
            return Err("资产对账余额证据时效配置无效");
        }
        Ok(())
    }
}

/// 一次完整对账的执行结果；任一步失败都通过 `ApplicationError` 明确返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetReconciliationOutcome {
    pub custody: CustodyReconciliationReport,
    pub solvency: SolvencyReport,
}

/// Worker 周期入口；只追加报告事实，不改变竞价、结算、余额或装备。
pub trait AssetReconciliationPort: Send + Sync {
    fn reconcile(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, AssetReconciliationOutcome>;
}
