//! 应用命令端口。
//! HTTP、Worker 和其他入口只依赖这些窄端口，不直接持有具体数据库事务或网络客户端。

use std::{cmp::Ordering, future::Future, pin::Pin};

use auction_domain::{
    BidAmount, CompetitionPlan, ProxyAuthorization, ProxyMode, ProxyStatus, ProxyTerminalReason,
    ProxyTransitionKind,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{ApplicationError, CreateProxyCommand, LowerProxyCommand, PlaceBidCommand};

/// 应用用例异步返回值，允许 HTTP state 持有动态端口而不暴露具体存储类型。
pub type ApplicationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// 按旧版 Go `time.Time` 的 `RFC3339Nano` 规则编码 Public 时间。
/// 领域与存储仍使用 `DateTime<Utc>`；只有对外 wire 去掉无意义的小数尾零，整秒不伪造
/// `.000000`，并继续用 `Z` 表示 UTC。
pub fn format_public_time_wire(value: DateTime<Utc>) -> String {
    let encoded = value.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let Some(body) = encoded.strip_suffix('Z') else { return encoded };
    let mut trimmed = body.trim_end_matches('0').trim_end_matches('.').to_owned();
    trimmed.push('Z');
    trimmed
}

/// 一次竞价命令冻结的时间 instant 与 Public wire 字符串。
/// `instant` 用于领域一致性、相等和顺序校验；`wire` 只用于重放旧 Go 命令账本时保留其
/// offset 与纳秒字符串。构造入口不向 crate 外开放，避免两者被独立拼装。
#[derive(Debug, Clone)]
pub struct CompetitionWireTimestamp {
    instant: DateTime<Utc>,
    wire: String,
}

impl CompetitionWireTimestamp {
    /// 从新版 UTC instant 生成旧版兼容 wire。
    pub(crate) fn from_utc(instant: DateTime<Utc>) -> Self {
        Self { instant, wire: format_public_time_wire(instant) }
    }

    /// 返回用于领域校验和时间运算的 UTC instant。
    pub fn instant(&self) -> DateTime<Utc> {
        self.instant
    }

    /// 返回冻结的 Public wire；旧账本重放不会在这里丢失原 offset 或纳秒。
    pub fn wire(&self) -> &str {
        &self.wire
    }
}

impl PartialEq for CompetitionWireTimestamp {
    fn eq(&self, other: &Self) -> bool {
        self.instant == other.instant
    }
}

impl Eq for CompetitionWireTimestamp {}

impl PartialOrd for CompetitionWireTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CompetitionWireTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        self.instant.cmp(&other.instant)
    }
}

impl Serialize for CompetitionWireTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.wire)
    }
}

impl<'de> Deserialize<'de> for CompetitionWireTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = String::deserialize(deserializer)?;
        let instant = DateTime::parse_from_rfc3339(&wire)
            .map_err(|_| de::Error::custom("旧版竞价时间不是合法 RFC3339"))?
            .with_timezone(&Utc);
        Ok(Self { instant, wire })
    }
}

/// 一次 Public 竞价命令在事务内冻结的完整结果。
/// `plan` 继续承载领域事实；代理专属投影直接来自同一命令账本结果，避免 HTTP 在事务提交后
/// 再读取账户投影并把已经成功的命令误报为失败。
#[derive(Debug, Clone)]
pub struct CompetitionCommandOutcome {
    pub plan: CompetitionPlan,
    pub proxy: Option<CompetitionProxyCommandOutcome>,
    /// 命令成功响应中的 bid 有效结束时间；无 bid 的 lower 响应仍冻结同一计划时间但不输出。
    pub effective_end_at: CompetitionWireTimestamp,
}

/// 旧 Public 代理成功响应所需、但不属于纯领域计划的冻结字段。
#[derive(Debug, Clone)]
pub struct CompetitionProxyCommandOutcome {
    pub authorization: ProxyAuthorization,
    pub quota_used: u64,
    pub quota_limit: u32,
    pub transitions: Vec<CompetitionProxyTransitionOutcome>,
    pub terminal_reason: Option<ProxyTerminalReason>,
}

/// 代理成功响应中的一条强类型转换事实。
/// 存储适配器可以使用数据库 wire 字符串，但应用命令端口只暴露已经严格校验的领域值，防止
/// 未知 mode/status/reason 穿透到 HTTP 成功响应。
#[derive(Debug, Clone)]
pub struct CompetitionProxyTransitionOutcome {
    pub transition_number: u64,
    pub kind: ProxyTransitionKind,
    pub old_maximum_amount: Option<BidAmount>,
    pub new_maximum_amount: BidAmount,
    pub old_mode: Option<ProxyMode>,
    pub new_mode: ProxyMode,
    pub old_status: Option<ProxyStatus>,
    pub new_status: ProxyStatus,
    pub reason: Option<ProxyTerminalReason>,
    pub resulting_state_version: u64,
    pub created_at: CompetitionWireTimestamp,
}

pub trait CompetitionCommandPort: Send + Sync {
    /// 执行普通竞价；幂等、锁和领域计划由应用服务与存储事务共同保证。
    fn place_bid(
        &self,
        command: PlaceBidCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome>;

    /// 创建代理授权；授权初始竞价与授权事实在同一事务中提交。
    fn create_proxy(
        &self,
        command: CreateProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome>;

    /// 降低活动代理上限；转换事实与授权快照在同一事务中提交。
    fn lower_proxy(
        &self,
        command: LowerProxyCommand,
    ) -> ApplicationFuture<'_, CompetitionCommandOutcome>;
}
