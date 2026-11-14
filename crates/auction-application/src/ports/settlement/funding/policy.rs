//! 数据库持久资金策略；仅账目与真实现金保护明确区分，不由进程默认值切换。

use serde::{Deserialize, Serialize};

use crate::ports::settlement::FundingFuture;

/// 旧账户默认保持真实现金门；record_only的来源与分录仅表达冻结业务账目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FundingAccountingMode {
    StrictCash,
    RecordOnly,
}

/// 在外部准备之前读取，最终事务仍在scope/account锁下重验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FundingPolicy {
    pub mode: FundingAccountingMode,
    pub expected_uid: Option<String>,
    pub policy_version: i64,
}

impl FundingPolicy {
    /// 仅strict模式请求账号余额；record_only仍获取联合身份与干净compose。
    pub fn observes_cash(&self) -> bool {
        self.mode == FundingAccountingMode::StrictCash
    }
    /// record_only只适用初始化时已核验的真实账号，不能借策略对错账号放行。
    pub fn matches_identity(&self, uid: &str) -> bool {
        self.expected_uid.as_deref().is_none_or(|expected| expected == uid)
    }
}

/// 全局mail_cookie只有一个record_only UID；无账户与旧strict均保持原有现金读取策略。
pub trait FundingPolicyReader: Send + Sync {
    fn funding_policy(&self) -> FundingFuture<'_, FundingPolicy>;
}
