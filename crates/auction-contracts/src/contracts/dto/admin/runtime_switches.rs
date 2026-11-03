//! 运行时开关只读 DTO；持久记录在 HTTP 边界显式映射。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 数据库最新意图，clear 与 expire 也保留版本及操作者。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSwitchIntentDto {
    /// 闭集开关名。
    pub name: String,
    /// 最新数据库版本；未设置为零。
    pub revision: i64,
    /// 最新动作；未设置为空。
    pub operation: Option<String>,
    /// 最新 set 的值。
    pub value: Option<Value>,
    /// UTC 期限。
    pub expires_at: Option<DateTime<Utc>>,
    /// 操作者声明。
    pub actor_id: Option<String>,
    /// 实际入口。
    pub entrypoint: Option<String>,
    /// 原因。
    pub reason: Option<String>,
}

/// Admin 当前实际使用的值，与数据库意图分开显示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSwitchAppliedDto {
    /// 名称。
    pub name: String,
    /// 实际观察版本。
    pub revision: i64,
    /// TOML 启动基线。
    pub toml_baseline: Value,
    /// 当前有效值。
    pub effective_value: Value,
    /// 是否使用数据库覆盖。
    pub overridden: bool,
}

/// 本接口只证明 Admin 自身应用情况，不冒充其他角色确认。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSwitchListResponse {
    /// 本次意图查询的数据库时间。
    pub database_time: DateTime<Utc>,
    /// 数据库最新事实投影。
    pub database_intent: Vec<RuntimeSwitchIntentDto>,
    /// Admin 最后成功快照年龄；空值为从未读取成功。
    pub admin_last_success_age_seconds: Option<f64>,
    /// Admin 实际值。
    pub admin_applied: Vec<RuntimeSwitchAppliedDto>,
}
