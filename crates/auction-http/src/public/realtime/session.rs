//! WebSocket 的只读会话复核；原始 token 仅留在连接内，不进入日志或事件。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::{sync::Arc, time::Duration};

use auction_application::public::{PublicSessionIdentity, PublicSessionPort};
use chrono::{DateTime, Utc};
use tokio::time::timeout;
use uuid::Uuid;

use super::super::is_unauthenticated;

const SESSION_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_CHECK_WAIT: Duration = Duration::from_secs(5);

pub(super) enum SessionCheckError {
    Expired,
    Unavailable,
}

/// 长连接固定绑定握手身份；只读复核不续期，但会跟随权威偏好变更更新下次截止检查。
pub(super) struct RealtimeSession {
    service: Arc<dyn PublicSessionPort>,
    raw_token: String,
    user_id: Uuid,
    expires_at: Option<DateTime<Utc>>,
}

impl RealtimeSession {
    pub(super) fn new(
        service: Arc<dyn PublicSessionPort>,
        raw_token: String,
        identity: PublicSessionIdentity,
    ) -> Self {
        Self {
            service,
            raw_token,
            user_id: identity.user_id,
            expires_at: identity.effective_expires_at(),
        }
    }

    /// 到期时重新读取，允许账号设置已修改期限；永久会话也定期复核撤销状态。
    pub(super) fn next_check_delay(&self) -> Duration {
        self.expires_at.map_or(SESSION_CHECK_INTERVAL, |expires_at| {
            (expires_at - Utc::now()).to_std().unwrap_or_default().min(SESSION_CHECK_INTERVAL)
        })
    }

    pub(super) async fn recheck(&mut self) -> Result<(), SessionCheckError> {
        let result =
            timeout(SESSION_CHECK_WAIT, self.service.inspect(self.raw_token.clone(), Utc::now()))
                .await;
        let identity = match result {
            Ok(Ok(identity)) => identity,
            Ok(Err(error)) if is_unauthenticated(&error) => {
                return Err(SessionCheckError::Expired);
            }
            Ok(Err(cause)) => {
                diagnostics::warn(
                    "public_realtime",
                    "read_session",
                    cause.diagnostic_kind(),
                    DiagnosticContext::default(),
                );
                return Err(SessionCheckError::Unavailable);
            }
            Err(cause) => {
                diagnostics::warn(
                    "public_realtime",
                    "read_session_timeout",
                    diagnostics::timeout_kind(&cause),
                    DiagnosticContext::default(),
                );
                return Err(SessionCheckError::Unavailable);
            }
        };
        let now = Utc::now();
        if identity.user_id != self.user_id
            || identity.expires_at.is_some() != identity.absolute_expires_at.is_some()
            || identity.expires_at.is_some_and(|expires_at| expires_at <= now)
            || identity.absolute_expires_at.is_some_and(|expires_at| expires_at <= now)
        {
            return Err(SessionCheckError::Expired);
        }
        self.expires_at = identity.effective_expires_at();
        Ok(())
    }
}
