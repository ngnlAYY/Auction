//! MoogleMail typed GET/POST 请求与远端结果分类。

use super::security::{
    is_hentaiverse_battle_response, is_html_content_type, is_session_expired, post_allowed,
};
use super::*;

impl MoogleMailClient {
    /// 对 typed target 执行一次受限 GET。
    ///
    /// 正文不进入错误对象或普通日志；异常响应只允许交给独立加密证据记录器。
    /// Cookie 仅在建立请求时短暂复制到 HTTP header，不参与响应留证。
    pub async fn get(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let mut response = DiagnosticResponse::default();
        match self.get_inner(target, cookie, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => Err(self
                .diagnose_http(target, response.status, response.body.as_deref(), error)
                .await),
        }
    }

    async fn get_inner(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
        if target.origin != self.builder.origin {
            return Err(self.configuration(target));
        }
        self.gate.wait().await.map_err(|_| {
            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;

        let request = self
            .client
            .get(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie.expose())
            .timeout(self.request_timeout)
            .build()
            .map_err(|_| self.configuration(target))?;
        let response = self.client.execute(request).await.map_err(|_| {
            MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;
        diagnostic.status = Some(response.status().as_u16());

        if response.url().as_str() != target.url.as_str() {
            return Err(self.protocol(target));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity")
            })
            || response
                .content_length()
                .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(self.response_too_large(target));
        }
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let is_html = is_html_content_type(response.headers());

        let body = time::timeout(self.request_timeout, response.bytes())
            .await
            .map_err(|_| {
                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            })?
            .map_err(|_| {
                MoogleMailError::new(
                    target.operation,
                    target.world,
                    MoogleMailErrorCategory::RemoteTemporary,
                    true,
                )
            })?;
        if body.len() > self.maximum_response_bytes {
            return Err(self.response_too_large(target));
        }
        diagnostic.body = Some(body.to_vec());

        if is_session_expired(status, location.as_ref(), &body) {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::SessionExpired,
                false,
            ));
        }
        if is_hentaiverse_battle_response(&body) {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::AccountBusy,
                true,
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RateLimited,
                true,
            ));
        }
        if status.is_server_error() {
            return Err(MoogleMailError::new(
                target.operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            ));
        }
        if !status.is_success() || !is_html {
            return Err(self.protocol(target));
        }
        diagnostic.body.take().ok_or_else(|| self.protocol(target))
    }

    /// 对 compose/detail typed target 执行恰好一次表单 POST。
    ///
    /// 请求开始后，无论是网络错误、响应结构错误还是远端明确拒绝，都不能证明远端没有
    /// 发生变化，因此统一返回 write_unknown，由上层进入人工对账或基于页面证明继续。
    pub async fn post(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let mut response = DiagnosticResponse::default();
        match self.post_inner(target, cookie, form, &mut response).await {
            Ok(body) => Ok(body),
            Err(error) => Err(self
                .diagnose_http(target, response.status, response.body.as_deref(), error)
                .await),
        }
    }

    async fn post_inner(
        &self,
        target: &MoogleMailTarget,
        cookie: &MoogleMailCookie,
        form: MoogleMailForm,
        diagnostic: &mut DiagnosticResponse,
    ) -> Result<Vec<u8>, MoogleMailError> {
        let operation = form.operation();
        if target.origin != self.builder.origin || !post_allowed(target.operation, operation) {
            return Err(self.configuration_for(operation, target.world));
        }
        self.gate.wait().await.map_err(|_| {
            MoogleMailError::new(
                operation,
                target.world,
                MoogleMailErrorCategory::RemoteTemporary,
                true,
            )
        })?;

        let encoded = SensitiveBytes(form.encode());
        let request = self
            .client
            .post(target.url.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(header::COOKIE, cookie.expose())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .timeout(self.request_timeout)
            .body(encoded.0.clone())
            .build()
            .map_err(|_| self.configuration_for(operation, target.world))?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|_| self.write_unknown(operation, target.world))?;
        diagnostic.status = Some(response.status().as_u16());

        if response.url().as_str() != target.url.as_str() {
            return Err(self.write_unknown(operation, target.world));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                !value.trim().is_empty() && !value.eq_ignore_ascii_case("identity")
            })
            || response
                .content_length()
                .is_some_and(|length| length > self.maximum_response_bytes as u64)
        {
            return Err(self.write_unknown(operation, target.world));
        }
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let is_html = is_html_content_type(response.headers());
        let body = time::timeout(self.request_timeout, response.bytes())
            .await
            .map_err(|_| self.write_unknown(operation, target.world))?
            .map_err(|_| self.write_unknown(operation, target.world))?;
        if body.len() > self.maximum_response_bytes {
            return Err(self.write_unknown(operation, target.world));
        }
        diagnostic.body = Some(body.to_vec());
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(MoogleMailError::after_write(
                operation,
                target.world,
                MoogleMailErrorCategory::SessionExpired,
            ));
        }
        if status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
            || !status.is_success()
            || !is_html
        {
            return Err(self.write_unknown(operation, target.world));
        }
        diagnostic.body.take().ok_or_else(|| self.write_unknown(operation, target.world))
    }

    fn configuration(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::Configuration,
            false,
        )
    }

    fn configuration_for(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::new(operation, world, MoogleMailErrorCategory::Configuration, false)
    }

    fn write_unknown(
        &self,
        operation: MoogleMailOperation,
        world: MoogleMailWorld,
    ) -> MoogleMailError {
        MoogleMailError::after_write(operation, world, MoogleMailErrorCategory::WriteUnknown)
    }

    fn protocol(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ProtocolDrift,
            false,
        )
    }

    fn response_too_large(&self, target: &MoogleMailTarget) -> MoogleMailError {
        MoogleMailError::new(
            target.operation,
            target.world,
            MoogleMailErrorCategory::ResponseTooLarge,
            false,
        )
    }
}

/// 响应头仅保留状态码；未取得完整、合规大小的正文时明确保持 None。
#[derive(Default)]
struct DiagnosticResponse {
    status: Option<u16>,
    body: Option<Vec<u8>>,
}

impl Drop for DiagnosticResponse {
    fn drop(&mut self) {
        if let Some(body) = &mut self.body {
            body.fill(0);
        }
    }
}

struct SensitiveBytes(Vec<u8>);

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
