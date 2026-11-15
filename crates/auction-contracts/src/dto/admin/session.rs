//! Admin 会话 DTO。

use serde::{Deserialize, Serialize};

/// Admin 会话响应；调试输出隐藏 CSRF token，授权响应保留原字段。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSessionResponse {
    pub authenticated: bool,
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

impl std::fmt::Debug for AdminSessionResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSessionResponse")
            .field("authenticated", &self.authenticated)
            .field("actor", &self.actor)
            .field("csrf_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("absolute_expires_at", &self.absolute_expires_at)
            .finish()
    }
}
