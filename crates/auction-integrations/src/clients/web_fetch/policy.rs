//! 取回限额与所请求的表示。

use std::time::Duration;

/// 单次取回的硬限额；超限一律失败，不截断。
#[derive(Debug, Clone, Copy)]
pub struct WebFetchPolicy {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// 单个响应体的字节上限，与挂载加载端的单文件上限同量级。
    pub max_file_bytes: u64,
}

impl Default for WebFetchPolicy {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(60),
            max_file_bytes: 64 << 20,
        }
    }
}

/// 请求哪一种表示：原文（`Accept-Encoding: identity`）或 Brotli 实体（`Accept-Encoding: br`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRepresentation {
    Identity,
    Brotli,
}

impl WebRepresentation {
    pub(super) const fn accept_encoding(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Brotli => "br",
        }
    }
}
