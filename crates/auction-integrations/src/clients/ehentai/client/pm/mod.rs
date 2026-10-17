//! PM 准备、单次提交和同账号 Sent 对账；令牌与 Cookie 只驻留本次内存会话。

use auction_application::{
    ForumPmError, ForumPmFuture, ForumPmMessage, ForumPmReceipt, PreparedForumPm,
};

use super::super::credentials::EhentaiCredentialSession;
use super::*;

mod form;
mod protocol;
mod sent;

use form::PmForm;
use protocol::{selector, sender_identity, validate_message};

// 全次扫描预算独立于每响应上限；Worker 另以 item timeout 包住准备、发送及回读。
const MAXIMUM_SCAN_PAGES: usize = 8;
const MAXIMUM_SCAN_BYTES: usize = 8 * 1024 * 1024;
const MAXIMUM_CANDIDATES: usize = 8;

/// 不实现 Debug/序列化，避免表单令牌经通用诊断泄漏；消费后不能再次 POST。
struct PreparedPm {
    client: EhentaiClient,
    session: EhentaiCredentialSession,
    message: ForumPmMessage,
    sender: String,
    form: PmForm,
}

impl PreparedForumPm for PreparedPm {
    fn sender_forum_account_id(&self) -> &str {
        &self.sender
    }

    fn send(self: Box<Self>) -> ForumPmFuture<'static, ForumPmReceipt> {
        Box::pin(async move {
            // 调用方必须先持久化 sending fence；本对象不存在重试、换号或重新准备分支。
            self.client
                .submit_pm(&self.form, self.session.cookie())
                .await
                .map_err(|_| ForumPmError::OutcomeUnknown)?;
            self.client
                .scan_sent(&self.message, &self.sender, self.session.cookie())
                .await
                .map_err(|_| ForumPmError::OutcomeUnknown)?
                .ok_or(ForumPmError::OutcomeUnknown)
        })
    }
}

impl EhentaiClient {
    pub(in crate::clients::ehentai) async fn prepare_pm(
        &self,
        message: &ForumPmMessage,
        mut session: EhentaiCredentialSession,
    ) -> Result<Box<dyn PreparedForumPm>, ForumPmError> {
        self.validate_pm_client()?;
        validate_message(message)?;
        let target = self.pm_url(&[
            ("act", "Msg"),
            ("CODE", "04"),
            ("MID", &message.recipient_forum_account_id),
        ])?;
        let result = self.pm_get(target.clone(), session.cookie()).await;
        if matches!(result, Err(ForumPmError::Authentication)) {
            let _ = session.invalidate_current().await;
        }
        let source = result?;
        let (sender, form) = form::parse(&source, &target, message)?;
        Ok(Box::new(PreparedPm {
            client: self.clone(),
            session,
            message: message.clone(),
            sender,
            form,
        }))
    }

    pub(in crate::clients::ehentai) async fn reconcile_pm(
        &self,
        message: &ForumPmMessage,
        sender: &str,
        mut session: EhentaiCredentialSession,
    ) -> Result<Option<ForumPmReceipt>, ForumPmError> {
        self.validate_pm_client()?;
        validate_message(message)?;
        if !valid_topic_id(sender) {
            return Err(ForumPmError::Protocol);
        }
        let result = self.scan_sent(message, sender, session.cookie()).await;
        if matches!(result, Err(ForumPmError::Authentication)) {
            let _ = session.invalidate_current().await;
        }
        result
    }

    pub(super) fn validate_pm_client(&self) -> Result<(), ForumPmError> {
        let production = Url::parse(PRODUCTION_ENDPOINT).map_err(|_| ForumPmError::Protocol)?;
        let allowed = same_origin(&self.base_url, &production)
            || (self.base_url.scheme() == "http" && is_loopback_host(&self.base_url));
        if !self.valid_for()
            || !allowed
            || !self.base_url.username().is_empty()
            || self.base_url.password().is_some()
            || self.base_url.query().is_some()
            || self.base_url.fragment().is_some()
            || self.base_url.path() != "/"
        {
            return Err(ForumPmError::Protocol);
        }
        if self.pm_client.is_none() {
            return Err(ForumPmError::Unavailable);
        }
        Ok(())
    }

    fn pm_url(&self, pairs: &[(&str, &str)]) -> Result<Url, ForumPmError> {
        self.index_url(pairs, EhentaiOperation::CheckSession).map_err(map_error)
    }

    // 目标资料只读路径复用安全 GET 连接池与 gate/owner，不创建 PM 操作。
    pub(super) async fn pm_get(
        &self,
        target: Url,
        cookie: &EhentaiCookie,
    ) -> Result<String, ForumPmError> {
        self.wait_rate(EhentaiOperation::CheckSession).await.map_err(map_error)?;
        self.check_ownership().await.map_err(|_| ForumPmError::Unavailable)?;
        let response = self
            .pm_client
            .as_ref()
            .ok_or(ForumPmError::Unavailable)?
            .get(target)
            .header(header::USER_AGENT, USER_AGENT)
            .header(
                header::COOKIE,
                cookie_header(cookie, EhentaiOperation::CheckSession).map_err(map_error)?,
            )
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| ForumPmError::Unavailable)?;
        let status = response.status();
        let location = response.headers().get(header::LOCATION).cloned();
        let html =
            response.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(
                |v| v.split(';').next().is_some_and(|v| v.trim().eq_ignore_ascii_case("text/html")),
            );
        let body = self
            .read_response(response, EhentaiOperation::CheckSession, false)
            .await
            .map_err(map_error)?;
        if is_session_expired(status, location.as_ref(), &body) {
            return Err(ForumPmError::Authentication);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(ForumPmError::RateLimited);
        }
        if !status.is_success() || location.is_some() || !html {
            return Err(ForumPmError::Protocol);
        }
        String::from_utf8(body).map_err(|_| ForumPmError::Protocol)
    }

    async fn submit_pm(&self, form: &PmForm, cookie: &EhentaiCookie) -> Result<(), ForumPmError> {
        self.wait_rate(EhentaiOperation::CheckSession).await.map_err(map_error)?;
        self.check_ownership().await.map_err(|_| ForumPmError::Unavailable)?;
        // 复用已有 URL 编码器，不为 PM 额外开启 reqwest feature。
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form.values.iter().map(|(name, value)| (name.as_str(), value.as_str())))
            .finish();
        // 只使用封闭白名单字段，不跟踪、不保存草稿；一次 send 调用仅有这一次 POST。
        let response = self
            .pm_client
            .as_ref()
            .ok_or(ForumPmError::Unavailable)?
            .post(form.action.clone())
            .header(header::USER_AGENT, USER_AGENT)
            .header(
                header::COOKIE,
                cookie_header(cookie, EhentaiOperation::CheckSession).map_err(map_error)?,
            )
            .header(header::ORIGIN, &self.origin)
            .header(header::REFERER, form.referer.as_str())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(encoded)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|_| ForumPmError::OutcomeUnknown)?;
        let accepted =
            response.status().is_success() && !response.headers().contains_key(header::LOCATION);
        self.read_response(response, EhentaiOperation::CheckSession, true)
            .await
            .map_err(|_| ForumPmError::OutcomeUnknown)?;
        if !accepted {
            return Err(ForumPmError::OutcomeUnknown);
        }
        // HTTP 200 只是允许开始只读核对，并非发送回执。
        Ok(())
    }
}

fn map_error(error: EhentaiError) -> ForumPmError {
    match error.category() {
        EhentaiErrorCategory::SessionExpired => ForumPmError::Authentication,
        EhentaiErrorCategory::RateLimited => ForumPmError::RateLimited,
        EhentaiErrorCategory::CredentialUnavailable | EhentaiErrorCategory::RemoteTemporary => {
            ForumPmError::Unavailable
        }
        _ => ForumPmError::Protocol,
    }
}
