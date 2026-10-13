//! WebSocket 的只读会话复核；原始 token 仅留在连接内，不进入日志或事件。

use std::{sync::Arc, time::Duration};

use auction_application::{PublicSessionIdentity, PublicSessionPort};
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

/// 长连接固定绑定握手身份；之后的复核只确认权威状态，不替换身份或刷新过期时间。
pub(super) struct RealtimeSession {
    service: Arc<dyn PublicSessionPort>,
    raw_token: String,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
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
            expires_at: identity.expires_at.min(identity.absolute_expires_at),
        }
    }

    /// 到期时重新读取，允许真实 HTTP 活动已延长会话；不能仅凭旧快照关闭活跃用户。
    pub(super) fn next_check_delay(&self) -> Duration {
        (self.expires_at - Utc::now()).to_std().unwrap_or_default().min(SESSION_CHECK_INTERVAL)
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
            Ok(Err(_)) | Err(_) => return Err(SessionCheckError::Unavailable),
        };
        let now = Utc::now();
        if identity.user_id != self.user_id
            || identity.expires_at <= now
            || identity.absolute_expires_at <= now
        {
            return Err(SessionCheckError::Expired);
        }
        self.expires_at = identity.expires_at.min(identity.absolute_expires_at);
        Ok(())
    }
}
