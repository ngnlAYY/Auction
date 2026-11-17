//! 一条告警 signal：稳定 fingerprint 生成、持久化前后的边界校验与单行通知摘要。

use std::time::Duration;

use serde::Serialize;

use super::super::digest::{signal_fingerprint, valid_signal_identity};
use super::errors::OperationsAlertError;
use super::kind::OperationsAlertKind;
use super::severity::OperationsAlertSeverity;
use super::source::OperationsAlertSource;

/// 一条稳定、无敏感内容的告警 signal。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationsAlertSignal {
    pub fingerprint: [u8; 32],
    pub kind: OperationsAlertKind,
    pub scope: String,
    pub source: OperationsAlertSource,
    pub count: i64,
    pub age_seconds: i64,
}

impl OperationsAlertSignal {
    /// 按 kind/scope 生成稳定 fingerprint，并校验 source 归属闭集。
    pub fn new(
        kind: OperationsAlertKind,
        scope: impl Into<String>,
        source: OperationsAlertSource,
        count: i64,
        age_seconds: i64,
    ) -> Result<Self, OperationsAlertError> {
        let scope = scope.into();
        let signal = Self {
            fingerprint: signal_fingerprint(kind, &scope),
            kind,
            scope,
            source,
            count,
            age_seconds,
        };
        signal.validate()?;
        Ok(signal)
    }

    /// 返回该 signal 的告警分级。
    pub const fn severity(&self) -> OperationsAlertSeverity {
        self.kind.severity()
    }

    /// 返回该 signal 下一次重复通知的最小间隔。
    pub fn repeat_interval(&self, cooldown: Duration) -> Duration {
        self.kind.repeat_interval(cooldown)
    }

    /// 校验持久化前后的 signal identity 与数值边界。
    pub fn validate(&self) -> Result<(), OperationsAlertError> {
        if self.count < 0
            || self.age_seconds < 0
            || !auction_domain::valid_text(&self.scope, 128)
            || self.fingerprint != signal_fingerprint(self.kind, &self.scope)
            || !valid_signal_identity(self.kind, &self.scope, self.source)
        {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(())
    }

    /// 生成不含 ID、收件人、原始错误或凭据的单行通知摘要。
    pub fn render_summary(&self) -> Result<String, OperationsAlertError> {
        self.validate()?;
        let summary = match self.kind {
            OperationsAlertKind::ServiceMissing => format!("{} 服务缺失", self.scope),
            OperationsAlertKind::ServiceDegraded => format!("{} 服务降级", self.scope),
            OperationsAlertKind::AuditFailure => format!("审计失败累计 {}", self.count),
            OperationsAlertKind::SourceUnavailable => format!("{} 健康来源不可用", self.scope),
            OperationsAlertKind::DeliveryPending => {
                format!("交付积压 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::DeliveryReturnedMail => format!(
                "MoogleMail 退信待取回 {}，最长 {} 秒；请核对后台交付页和 MM",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::DeliveryManualReconcile => {
                format!("交付人工对账 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::FulfillmentBacklog => {
                format!("结算履约积压 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::FulfillmentFailed => format!("结算履约失败 {}", self.count),
            OperationsAlertKind::FulfillmentManualReconcile => {
                format!("结算履约人工对账 {}", self.count)
            }
            OperationsAlertKind::FulfillmentCollectionOverdue => format!(
                "长期未领取：{} 笔寄出时长已达阈值仍未领取，最久已寄出 {} 天；\
                 只提醒，不会重发或撤回，请在游戏内核对 Sent 并联系收件人",
                self.count,
                self.age_seconds / 86_400
            ),
            OperationsAlertKind::SettlementReturnPending => format!(
                "结算退件待处理 {} 件，最长 {} 秒；请在 Admin 退件页决定领回补寄、领回代管或留箱",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::SettlementReturnQuarantined => format!(
                "结算退件隔离待核对 {} 件，最长 {} 秒；只能标记已核对或提交只读核验",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::SettlementReturnTakeUnknown => format!(
                "结算退件 Take 结果未知 {} 件（原因 {}），最长 {} 秒；{}",
                self.count,
                self.scope,
                self.age_seconds,
                take_unknown_guidance(&self.scope)
            ),
            OperationsAlertKind::RecoveryPaymentOverdue => {
                format!("结算付款逾期 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::RecoveryPaymentMailUnsent => format!(
                "付款邮件未寄出但已逾期 {} 封，最长 {} 秒；请先核对付款投递并人工处理",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::RecoveryRelistPending => format!("结算恢复待重挂 {}", self.count),
            OperationsAlertKind::RecoveryFailed => format!("结算恢复失败 {}", self.count),
            OperationsAlertKind::RecoveryManualReconcile => {
                format!("结算恢复人工对账 {}", self.count)
            }
            OperationsAlertKind::BalanceLow => {
                format!("{} 结算余额偏低，当前 {}", self.scope, self.count)
            }
            OperationsAlertKind::EvidenceStale => {
                format!("{} 恢复证据过期 {} 秒", self.scope, self.age_seconds)
            }
            OperationsAlertKind::MailPollStalled => format!(
                "MoogleMail 收件轮询停滞 {} 秒，连续失败 {}；请检查收件凭据与邮箱页",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::MailSentPageLimitApproaching => {
                format!("Sent 列表页数接近上限：{}", self.count)
            }
            OperationsAlertKind::CredentialInvalid => {
                format!("{} 当前凭据失效，请重新录入", self.scope)
            }
            OperationsAlertKind::AuctionPhaseOverdue => format!(
                "场次结束 {} 秒后仍未进入 Final，共 {} 个；请检查 Worker 生命周期",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::ProxyExpiryOverdue => format!(
                "慢速代理到期 {} 秒后仍未执行，共 {} 条；请检查 Worker 与代理任务",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::QueueLatency => format!(
                "投递队列延迟 {} 秒仍未推进，共 {} 条；请检查 Worker 领取",
                self.age_seconds, self.count
            ),
            OperationsAlertKind::ForumSyncFailed => {
                format!("论坛主题发布失败 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::ForumMessageFailed => format!(
                "论坛私信失败 {}，最长 {} 秒；请核对发送账号与私信页",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::ForumTopicPageLimitApproaching => {
                format!("论坛主题远端页数接近上限：{}", self.count)
            }
            OperationsAlertKind::NotificationDeliveryFailed => {
                format!("非结算通知投递失败 {}，最长 {} 秒", self.count, self.age_seconds)
            }
            OperationsAlertKind::NotificationPostageInsufficient => {
                format!("非结算通知邮资不足 {}；请补充邮资后重试", self.count)
            }
            OperationsAlertKind::AccountBusy => format!(
                "{} 外部账号已忙 {} 秒，共 {} 次观测",
                self.scope, self.age_seconds, self.count
            ),
            OperationsAlertKind::DiskCapacityLow => format!(
                "备份文件系统可用空间低于配置水位，缺口 {} 字节；请清理旧副本或扩容",
                self.count
            ),
            OperationsAlertKind::BackupFailed => format!(
                "定时备份最近一次运行未成功，已 {} 秒；请核对回执与 journal",
                self.age_seconds
            ),
            OperationsAlertKind::RestoreDrillOverdue if self.count == 0 => {
                "尚无成功的恢复演练记录；请按恢复手册在隔离实例完成一次演练并留存回执".to_owned()
            }
            OperationsAlertKind::RestoreDrillOverdue => format!(
                "最近一次成功的恢复演练已 {} 天；请按恢复手册完成月度演练并留存回执",
                self.age_seconds / 86_400
            ),
            OperationsAlertKind::WalUploadStalled => format!(
                "WAL 归档暂存积压 {} 段，最老 {} 秒未上传；请核对上传器与 PostgreSQL 回收",
                self.count, self.age_seconds
            ),
            OperationsAlertKind::SeasonObligationsPending => {
                format!("换季前未完成 isekai 义务：{}", self.count)
            }
            OperationsAlertKind::CustodyDrift => format!(
                "{} 装备对账差异 {} 件，报告年龄 {} 秒；请核对 Armory 与义务，不自动转移",
                self.scope, self.count, self.age_seconds
            ),
            OperationsAlertKind::SolvencyShortfall => format!(
                "{} 偿付缺口 {} credits，报告年龄 {} 秒；请先核对余额与待付货款，不自动转账",
                self.scope, self.count, self.age_seconds
            ),
            OperationsAlertKind::ReconciliationStale => format!(
                "{} 资产报告已 {} 秒未更新，请检查 Worker 对账",
                self.scope, self.age_seconds
            ),
        };
        let summary = format!("{summary}；处置手册：{}", self.source.runbook_anchor());
        if !auction_domain::valid_text(&summary, 512) {
            return Err(OperationsAlertError::Invalid);
        }
        Ok(summary)
    }
}

/// Take 结果未知各原因的单行处置指引；与 Admin 退件详情的说明同一口径。作用域已由身份校验限定
/// 在原因闭集内，未知名称不会到达这里。
fn take_unknown_guidance(scope: &str) -> &'static str {
    use auction_domain::SettlementReturnTakeUnknownReason as Reason;

    match Reason::from_name(scope) {
        Some(Reason::AssetsClaimable) => {
            "原资产完整仍在且可领取，自动重排已用满：在退件详情“再次领回”"
        }
        Some(Reason::AssetsUnclaimable) => {
            "原资产仍在但当前不可领取：等待巡检再次观察，不要手工领取"
        }
        Some(Reason::AssetsChanged) => {
            "邮件资产与冻结资产不符：在游戏内核对原退件与资产对账报告后上报，不要人工改库"
        }
        Some(Reason::MailMissing) => {
            "原退件已不在收件箱：无法证明资产入账，核对资产对账报告后上报，不要人工声明"
        }
        Some(Reason::ReceiptUnproven) => {
            "附件已空但领取回执不成立：核对资产对账报告后上报，不要人工声明"
        }
        Some(Reason::SessionChanged) => "原 Take 凭据已停用或换版本：恢复原凭据后巡检自动复核",
        Some(Reason::TakeUnverifiable) => {
            "Take 投递以失败或回执不符终结、巡检不复核：在游戏内核对原退件后上报"
        }
        Some(Reason::DispositionConflict) => "同邮件有其他处置意图：等其确定从未执行后自动交接",
        Some(Reason::AwaitingObservation) | None => "等待邮箱维护在原 Take 会话复核",
    }
}
