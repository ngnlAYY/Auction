//! MoogleMail 单封业务 envelope 的有界重试与安全失败分类。

use auction_application::MailProcessingRetryability;
use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_observability::WorkerRetryKind;

use super::*;
use crate::worker::diagnostics;

pub(super) struct MailProcessFailure {
    pub(super) failure_code: &'static str,
    pub(super) safe_detail: &'static str,
}

pub(super) async fn process_with_retry(
    processor: &dyn MailProcessor,
    business: MailMessage,
    config: MailPollRuntimeConfig,
    metric_context: MailPollMetricContext,
    pending_diagnostics: &mut Vec<auction_application::PendingMailEquipmentDiagnostic>,
    decision_diagnostic: &mut Option<usize>,
) -> Result<MailProcessingDecision, MailProcessFailure> {
    let mut original = Some(business);
    for attempt in 1..=config.process_attempts {
        // 非最终尝试只建立一个显式短生命周期副本；成功或失败返回时，未使用的原 envelope 会清零。
        let message = if attempt == config.process_attempts {
            let Some(message) = original.take() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message
        } else {
            let Some(message) = original.as_ref() else {
                return Err(MailProcessFailure {
                    failure_code: "business_retry_state_invalid",
                    safe_detail: "MoogleMail 业务重试状态无效",
                });
            };
            message.duplicate_for_retry()
        };
        let result = tokio::time::timeout(config.item_timeout, processor.process(message)).await;
        let result = match result {
            Ok(mut attempt) => {
                // 这里只转移句柄；留证由 caller 在单封和批次两层业务 timeout 之外执行。
                if let Some(pending) = attempt.pending_equipment_diagnostic.take() {
                    if attempt.result.is_ok() {
                        *decision_diagnostic = Some(pending_diagnostics.len());
                    }
                    pending_diagnostics.push(pending);
                }
                Ok(attempt.result)
            }
            Err(elapsed) => Err(elapsed),
        };
        match result {
            Ok(Ok(decision)) => return Ok(decision),
            Ok(Err(error)) => {
                diagnostics::warn(
                    "mail_poll",
                    "process_business",
                    error.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                match error.mail_processing_retryability() {
                    MailProcessingRetryability::Retryable if attempt < config.process_attempts => {
                        metric_context
                            .metrics
                            .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                        tokio::time::sleep(retry_delay(
                            config.retry_base,
                            config.retry_maximum,
                            u32::from(attempt),
                        ))
                        .await;
                    }
                    MailProcessingRetryability::Retryable => {
                        return Err(MailProcessFailure {
                            failure_code: "business_processing_failed",
                            safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
                        });
                    }
                    MailProcessingRetryability::Permanent => {
                        return Err(MailProcessFailure {
                            failure_code: "business_processing_permanent",
                            safe_detail: "MoogleMail 业务处理遇到确定性失败，入站游标保持不变",
                        });
                    }
                }
            }
            Err(error) if attempt < config.process_attempts => {
                diagnostics::warn(
                    "mail_poll",
                    "retry_business_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                metric_context
                    .metrics
                    .observe_retry(metric_context.loop_kind, WorkerRetryKind::Inline);
                tokio::time::sleep(retry_delay(
                    config.retry_base,
                    config.retry_maximum,
                    u32::from(attempt),
                ))
                .await;
            }
            Err(error) => {
                diagnostics::warn(
                    "mail_poll",
                    "business_timeout",
                    diagnostics::timeout_kind(&error),
                    DiagnosticContext::default(),
                );

                return Err(MailProcessFailure {
                    failure_code: "business_processing_timeout",
                    safe_detail: "MoogleMail 单封邮件处理在总时限内未完成",
                });
            }
        }
    }
    Err(MailProcessFailure {
        failure_code: "business_processing_failed",
        safe_detail: "MoogleMail 业务处理未完成，入站游标保持不变",
    })
}
