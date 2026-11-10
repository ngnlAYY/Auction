//! 外部账号当前进程观测的只读投影。
//! 只包含 Worker 已观察到的闭集事实：邮件/论坛请求时间、请求门等待、维护轮次结果与账号忙碌
//! 时长。模型不含凭据、Cookie、远端 URL、页面正文或游标；`None` 表示本进程尚未观察到，
//! 不能被解读为“未知的成功”或“空闲”。读取器只服务只读 Admin 诊断，不参与任何业务裁决。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 一次 MoogleMail 维护轮次的结束状态；只描述本进程观察到的轮次结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalMaintenanceState {
    /// 本进程尚未结束任何维护轮次。
    #[default]
    Idle,
    /// 至少一个世界的维护轮次仍在执行。
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl ExternalMaintenanceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

/// 邮件账号的进程内观测；计数字段自本进程启动起累计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalMailAccountStatus {
    /// 外部账号当前是否报告 `account_busy`；两个世界合并观察。
    pub busy: bool,
    /// 自最近一次 `account_busy` 起、尚未被成功请求清除的持续秒数；仅在 `busy` 时存在。
    pub busy_age_seconds: Option<i64>,
    /// 请求门等待次数，包含获准、失败与取消。
    pub gate_wait_count: i64,
    /// 请求门等待累计整秒，包含获准、失败与取消。
    pub gate_wait_seconds_total: i64,
    /// 已开始的物理邮件请求数。
    pub request_count: i64,
    pub last_request_at: Option<DateTime<Utc>>,
    /// 最近一次边界结果为 `ok` 的请求时间；`canceled` 不改变成功/失败。
    pub last_success_at: Option<DateTime<Utc>>,
    /// 最近一次边界结果为非 `ok` 且非 `canceled` 的请求时间。
    pub last_failure_at: Option<DateTime<Utc>>,
}

/// 论坛账号的进程内观测；只保留请求边界时间与写入未知计数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalForumAccountStatus {
    pub last_request_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    /// 论坛写入结果未知次数；未知不等于失败，也不授权自动重发。
    pub write_unknown_count: i64,
}

/// 邮件维护的进程内观测；两个世界合并为一个最近状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalMaintenanceStatus {
    pub state: ExternalMaintenanceState,
    pub last_started_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_canceled_at: Option<DateTime<Utc>>,
    /// 最早已在途维护轮次的持续秒数；仅在 `Running` 时存在。
    pub running_age_seconds: Option<i64>,
}

/// Worker 当前的只读外部账号快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAccountStatus {
    /// 本快照生成时刻；所有可选时间都不得晚于该时刻。
    pub observed_at: DateTime<Utc>,
    pub mail: ExternalMailAccountStatus,
    pub forum: ExternalForumAccountStatus,
    pub maintenance: ExternalMaintenanceStatus,
}

impl ExternalAccountStatus {
    /// 校验计数、可选时间与维护状态之间的字段关联；不合法快照不得进入 Admin。
    pub fn validate(&self) -> bool {
        self.observed_at.timestamp_nanos_opt().is_some()
            && valid_time(self.mail.last_request_at, self.observed_at)
            && valid_time(self.mail.last_success_at, self.observed_at)
            && valid_time(self.mail.last_failure_at, self.observed_at)
            && valid_time(self.forum.last_request_at, self.observed_at)
            && valid_time(self.forum.last_success_at, self.observed_at)
            && valid_time(self.forum.last_failure_at, self.observed_at)
            && valid_time(self.maintenance.last_started_at, self.observed_at)
            && valid_time(self.maintenance.last_success_at, self.observed_at)
            && valid_time(self.maintenance.last_failure_at, self.observed_at)
            && valid_time(self.maintenance.last_canceled_at, self.observed_at)
            && self.mail.gate_wait_count >= 0
            && self.mail.gate_wait_seconds_total >= 0
            && self.mail.request_count >= 0
            && self.forum.write_unknown_count >= 0
            && self.mail.busy_age_seconds.is_none_or(|age| age >= 0)
            && (self.mail.busy == self.mail.busy_age_seconds.is_some())
            && self.maintenance.running_age_seconds.is_none_or(|age| age >= 0)
            && match self.maintenance.state {
                ExternalMaintenanceState::Idle => {
                    self.maintenance.last_started_at.is_none()
                        && self.maintenance.last_success_at.is_none()
                        && self.maintenance.last_failure_at.is_none()
                        && self.maintenance.last_canceled_at.is_none()
                        && self.maintenance.running_age_seconds.is_none()
                }
                ExternalMaintenanceState::Running => {
                    self.maintenance.last_started_at.is_some()
                        && self.maintenance.running_age_seconds.is_some()
                }
                ExternalMaintenanceState::Succeeded => {
                    self.maintenance.last_started_at.is_some()
                        && self.maintenance.last_success_at.is_some()
                        && self.maintenance.running_age_seconds.is_none()
                }
                ExternalMaintenanceState::Failed => {
                    self.maintenance.last_started_at.is_some()
                        && self.maintenance.last_failure_at.is_some()
                        && self.maintenance.running_age_seconds.is_none()
                }
                ExternalMaintenanceState::Canceled => {
                    self.maintenance.last_started_at.is_some()
                        && self.maintenance.last_canceled_at.is_some()
                        && self.maintenance.running_age_seconds.is_none()
                }
            }
    }
}

fn valid_time(value: Option<DateTime<Utc>>, observed_at: DateTime<Utc>) -> bool {
    value.is_none_or(|value| value.timestamp_nanos_opt().is_some() && value <= observed_at)
}

/// Worker 进程内外部账号观测的只读读取器。
/// 返回 `None` 表示该进程没有可用的运行时快照（关闭指标、角色未装配或观测 owner 不可用），
/// 调用方必须显式降级，不能把 `None` 当作空闲。
pub trait ExternalAccountStatusReader: Send + Sync {
    fn snapshot(&self) -> Option<ExternalAccountStatus>;
}
