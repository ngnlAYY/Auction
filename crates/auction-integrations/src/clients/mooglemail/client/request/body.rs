//! 有界响应正文读取与敏感缓冲：逐块计量字节上限，失败、取消或超时都清零半截正文。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use tokio::time;

use super::super::MoogleMailClient;
use crate::clients::metrics::mail_signal;
use crate::clients::mooglemail::{
    MoogleMailError, MoogleMailErrorCategory, MoogleMailOperation, MoogleMailWorld,
};
use crate::safe_diagnostics;

impl MoogleMailClient {
    /// 每块正文入缓冲前校验总量；无 Content-Length 的响应也不能绕过内存边界。
    pub(in crate::clients::mooglemail::client) async fn read_response_body(
        &self,
        mut response: reqwest::Response,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
        write_started: bool,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let read_error = || {
            if write_started {
                self.write_unknown(operation, world)
            } else {
                MoogleMailError::new(
                    operation,
                    world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            }
        };
        let capacity = response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(self.maximum_response_bytes);
        time::timeout(self.request_timeout, async {
            // 有界预留减少敏感正文扩容复制；失败或取消清理半截正文，诊断仅保留完整响应。
            let mut body = SensitiveBytes(Vec::with_capacity(capacity));
            while let Some(chunk) = response.chunk().await.map_err(|diagnostic_cause| {
                if diagnostic_cause.is_timeout() {
                    mail_signal(
                        self.external_metrics.as_ref(),
                        world,
                        operation,
                        auction_application::external_metrics::MailSignal::Timeout,
                    );
                }
                safe_diagnostics::warn(
                    "mooglemail_read",
                    "read_response_chunk",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        write_started: Some(write_started),
                        ..Default::default()
                    },
                );
                read_error()
            })? {
                if body.0.len().saturating_add(chunk.len()) > self.maximum_response_bytes {
                    return Err(if write_started {
                        self.write_unknown(operation, world)
                    } else {
                        MoogleMailError::new(
                            operation,
                            world,
                            MoogleMailErrorCategory::ResponseTooLarge,
                            false,
                        )
                    });
                }
                body.0.extend_from_slice(&chunk);
            }
            Ok(std::mem::take(&mut body.0))
        })
        .await
        .map_err(|diagnostic_cause| {
            mail_signal(
                self.external_metrics.as_ref(),
                world,
                operation,
                auction_application::external_metrics::MailSignal::Timeout,
            );
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_response_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext {
                    world: diagnostic_world(world.as_str()),
                    write_started: Some(write_started),
                    ..Default::default()
                },
            );
            read_error()
        })?
    }
}

/// 响应头仅保留状态码；未取得完整、合规大小的正文时明确保持 None。
#[derive(Default)]
pub(in crate::clients::mooglemail::client) struct DiagnosticResponse {
    pub(in crate::clients::mooglemail::client) status: Option<u16>,
    pub(in crate::clients::mooglemail::client) body: Option<Vec<u8>>,
}

impl Drop for DiagnosticResponse {
    fn drop(&mut self) {
        if let Some(body) = &mut self.body {
            body.fill(0);
        }
    }
}

/// 敏感正文缓冲；失败、取消或超时时随 Drop 清零。
pub(in crate::clients::mooglemail::client) struct SensitiveBytes(
    pub(in crate::clients::mooglemail::client) Vec<u8>,
);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
