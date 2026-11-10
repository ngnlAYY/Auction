//! Worker control 的应用端口与安全状态投影。
//! Unix socket、HTTP/JSON 和进程凭据校验属于外层适配器；应用层只接收有界的运行状态，避免
//! Admin dashboard 直接依赖 Worker 的网络实现。手动轮询是唤醒提示，不替代邮件命令账本、
//! 游标或入站事务。

pub(crate) mod protocol;
pub(crate) mod remote_read;

use crate::ports::auction::ApplicationFuture;
use crate::ports::external_accounts::ExternalAccountStatus;
use crate::ports::mail::{MAX_MAIL_PROGRESS_POLLERS, MailPollProgressSnapshot};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Worker control client 读取失败时向 Admin 暴露的有限分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlCondition {
    /// Worker control 返回了符合当前协议的状态。
    Available,
    /// socket 不存在、连接失败或对端主动不可用。
    Unavailable,
    /// Worker control 请求超过了有界超时。
    Timeout,
    /// 对端协议版本或响应形状不受支持。
    Incompatible,
    /// 对端明确拒绝了请求。
    Rejected,
}

impl WorkerControlCondition {
    /// 返回 Admin/JSON 契约中的稳定条件名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Incompatible => "incompatible",
            Self::Rejected => "rejected",
        }
    }
}

/// Worker control 的不含敏感值状态摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlStatus {
    pub condition: WorkerControlCondition,
    pub state: String,
    pub ready: bool,
    pub version: String,
    pub mail_pollers: i64,
}

impl WorkerControlStatus {
    /// 构造失败状态；失败时清空对端字段，避免展示过期或未验证的 ready 信息。
    pub fn unavailable(condition: WorkerControlCondition) -> Self {
        Self {
            condition,
            state: String::new(),
            ready: false,
            version: String::new(),
            mail_pollers: 0,
        }
    }
}

/// Worker 手动唤醒的安全结果；唤醒失败不否定已经写入数据库的 due 命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlTrigger {
    pub condition: WorkerControlCondition,
    pub accepted: bool,
    pub pollers: i64,
}

/// Worker 邮件进度的有界读取结果；不可用时不返回旧观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlMailProgress {
    pub condition: WorkerControlCondition,
    pub observed_at: Option<DateTime<Utc>>,
    pub pollers: Vec<MailPollProgressSnapshot>,
}

impl WorkerControlMailProgress {
    /// 明确标识诊断不可用，不能把空集合解释为所有邮箱空闲。
    pub fn unavailable(condition: WorkerControlCondition) -> Self {
        Self { condition, observed_at: None, pollers: Vec::new() }
    }

    /// 校验可选能力与快照的字段关联、唯一性和数量边界。
    pub fn validate(&self) -> bool {
        if self.condition != WorkerControlCondition::Available {
            return self.observed_at.is_none() && self.pollers.is_empty();
        }
        let Some(observed_at) = self.observed_at else { return false };
        observed_at.timestamp_nanos_opt().is_some()
            && self.pollers.len() <= MAX_MAIL_PROGRESS_POLLERS
            && self.pollers.iter().enumerate().all(|(index, poller)| {
                poller.validate(observed_at)
                    && self.pollers[..index]
                        .iter()
                        .all(|previous| previous.mailbox_key != poller.mailbox_key)
            })
    }
}

/// Worker 外部账号运行时快照的有界读取结果；不可用时不返回旧观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerControlExternalAccounts {
    pub condition: WorkerControlCondition,
    pub status: Option<ExternalAccountStatus>,
}

impl WorkerControlExternalAccounts {
    /// 明确标识运行时快照不可用，不能把缺失解释为空闲。
    pub fn unavailable(condition: WorkerControlCondition) -> Self {
        Self { condition, status: None }
    }

    /// 校验条件与快照字段关联；`Available` 必须携带合法快照。
    pub fn validate(&self) -> bool {
        if self.condition != WorkerControlCondition::Available {
            return self.status.is_none();
        }
        self.status.as_ref().is_some_and(ExternalAccountStatus::validate)
    }
}

/// Admin dashboard 读取 Worker control 的窄端口。
pub trait WorkerControlPort: Send + Sync {
    fn status(&self) -> ApplicationFuture<'_, WorkerControlStatus>;

    /// 读取可选的邮件运行观测；旧适配器明确不可用，不假装空闲。
    fn mail_progress(&self) -> ApplicationFuture<'_, WorkerControlMailProgress> {
        Box::pin(async {
            Ok(WorkerControlMailProgress::unavailable(WorkerControlCondition::Unavailable))
        })
    }

    /// 读取可选的外部账号运行时快照；旧适配器明确不可用。
    fn external_accounts(&self) -> ApplicationFuture<'_, WorkerControlExternalAccounts> {
        Box::pin(async {
            Ok(WorkerControlExternalAccounts::unavailable(WorkerControlCondition::Unavailable))
        })
    }

    fn trigger_poll(&self, command_id: Uuid) -> ApplicationFuture<'_, WorkerControlTrigger>;
}
