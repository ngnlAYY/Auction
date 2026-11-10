//! Webhook 告警通道的独立持久账本边界；与 MoogleMail 通知互不消费冷却或投递状态。

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::signals::{OperationsAlertError, OperationsAlertKind};
use crate::ports::auction::ApplicationFuture;

/// 默认 webhook JSON 模板；占位符只允许出现在冻结投递中。
/// Webhook 可重试失败的最大尝试次数；耗尽后进入 failed/retry_exhausted。
pub const MAX_OPERATIONS_ALERT_WEBHOOK_ATTEMPTS: u32 = 5;

pub const DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE: &str = r#"{"format":"auction-operations-alert.v1","kind":"{kind}","scope":"{scope}","source":"{source}","count":{count},"age_seconds":{age_seconds}}"#;

/// 一次已经冻结并由 webhook 通道持有的投递。
#[derive(Debug, Clone)]
pub struct OperationsAlertWebhookDispatch {
    pub delivery_id: Uuid,
    pub command_id: Uuid,
    pub claim_token: Uuid,
    pub claim_fencing: i64,
    /// 本次 claim 已包含的尝试序号（首次为 1）。
    pub attempt_count: u32,
    pub kind: OperationsAlertKind,
    /// 创建投递时冻结的 JSON；发送时不得按当前配置重算。
    pub payload: Value,
}

/// webhook 发送结果闭集；Unknown 必须转人工，不能自动重发。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertWebhookOutcome {
    Succeeded,
    Retryable,
    Permanent,
    /// 可重试失败已达到尝试上限，转入 dead-letter。
    RetryExhausted,
    Unknown,
}

/// 创建 webhook 投递时的不可变请求；模板由非敏感配置提供。
#[derive(Debug, Clone)]
pub struct OperationsAlertWebhookClaimRequest {
    pub now: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub template: String,
}

/// 渲染并校验 webhook JSON；值只来自闭集或整数，避免模板注入任意凭据。
pub fn render_webhook_payload(
    template: &str,
    kind: OperationsAlertKind,
    scope: &str,
    source: &str,
    count: i64,
    age_seconds: i64,
) -> Result<Value, OperationsAlertError> {
    let rendered = template
        .replace("{kind}", kind.as_str())
        .replace("{scope}", scope)
        .replace("{source}", source)
        .replace("{count}", &count.to_string())
        .replace("{age_seconds}", &age_seconds.to_string());
    let value: Value = serde_json::from_str(&rendered)
        .map_err(|_: serde_json::Error| OperationsAlertError::Invalid)?;
    if !value.is_object() {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(value)
}

/// 校验模板在示例值下仍能生成 JSON 对象。
pub fn validate_webhook_template(template: &str) -> Result<(), OperationsAlertError> {
    if template.len() > 16 << 10 || template.trim().is_empty() {
        return Err(OperationsAlertError::Invalid);
    }
    render_webhook_payload(template, OperationsAlertKind::ServiceMissing, "sample", "sample", 0, 0)
        .map(|_| ())
}

/// 外部 webhook 通道端口；实现只能发送已冻结的 payload。
pub trait OperationsAlertWebhookSender: Send + Sync {
    /// `command_id` 必须作为外部幂等键传递；同一次重试不得生成新 ID。
    fn send(
        &self,
        payload: Value,
        command_id: Uuid,
    ) -> ApplicationFuture<'_, OperationsAlertWebhookOutcome>;
}

/// 发送结果与下一次该 signal 的 webhook 时间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationsAlertWebhookFinish {
    pub outcome: OperationsAlertWebhookOutcome,
    pub next_webhook_at: Option<DateTime<Utc>>,
}

impl OperationsAlertWebhookFinish {
    pub fn validate(self) -> Result<(), OperationsAlertError> {
        if self.next_webhook_at.is_some_and(|value| value.timestamp_nanos_opt().is_none()) {
            return Err(OperationsAlertError::Invalid);
        }
        match self.outcome {
            OperationsAlertWebhookOutcome::Succeeded | OperationsAlertWebhookOutcome::Retryable => {
                if self.next_webhook_at.is_none() {
                    return Err(OperationsAlertError::Invalid);
                }
            }
            OperationsAlertWebhookOutcome::Permanent
            | OperationsAlertWebhookOutcome::RetryExhausted
            | OperationsAlertWebhookOutcome::Unknown => {
                if self.next_webhook_at.is_some() {
                    return Err(OperationsAlertError::Invalid);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE, OperationsAlertKind, render_webhook_payload,
        validate_webhook_template,
    };

    #[test]
    fn default_template_renders_closed_values() {
        let payload = render_webhook_payload(
            DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE,
            OperationsAlertKind::ServiceMissing,
            "main",
            "process",
            3,
            42,
        )
        .expect("默认模板必须有效");
        assert_eq!(payload["kind"], "service_missing");
        assert_eq!(payload["source"], "process");
        assert_eq!(payload["count"], 3);
        assert_eq!(payload["age_seconds"], 42);
        assert!(validate_webhook_template(DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE).is_ok());
    }

    #[test]
    fn finish_requires_cooldown_for_retryable_and_none_for_terminal_unknown() {
        use super::{OperationsAlertWebhookFinish, OperationsAlertWebhookOutcome};
        use chrono::Utc;

        let now = Utc::now();
        assert!(
            OperationsAlertWebhookFinish {
                outcome: OperationsAlertWebhookOutcome::Retryable,
                next_webhook_at: None,
            }
            .validate()
            .is_err()
        );
        assert!(
            OperationsAlertWebhookFinish {
                outcome: OperationsAlertWebhookOutcome::Unknown,
                next_webhook_at: Some(now),
            }
            .validate()
            .is_err()
        );
        assert!(
            OperationsAlertWebhookFinish {
                outcome: OperationsAlertWebhookOutcome::Succeeded,
                next_webhook_at: Some(now),
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn non_object_or_invalid_template_is_rejected() {
        assert!(validate_webhook_template("[]").is_err());
        assert!(validate_webhook_template("{\"kind\":").is_err());
        assert!(validate_webhook_template("").is_err());
    }
}
