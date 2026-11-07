//! 单次论坛登录获取 Cookie；固定外部目标、无重试，并且只有只读会话证明成功后才返回凭据。

use auction_application::core::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use auction_application::admin::{
    AdminCredentialAcquirer, AdminCredentialError, AdminCredentialImportSource,
    AdminCredentialKind, AdminCredentialLogin, AdminCredentialSecret,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use reqwest::{Client, header, redirect::Policy};
use ring::hmac;
use tokio::sync::{Semaphore, SemaphorePermit};
use url::Url;

use crate::{
    DataKey,
    ehentai::{
        EhentaiClient, EhentaiClientConfig, EhentaiCookie, EhentaiErrorCategory,
        PRODUCTION_ENDPOINT,
    },
};

mod cookies;
mod form;
mod response;

const LOGIN_DIGEST_DOMAIN: &[u8] = b"auction/credential/forum-login-command/v1\0";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);
const LOGIN_COOLDOWN: Duration = Duration::from_secs(30);
const MAXIMUM_RESPONSE_BYTES: usize = 512 * 1024;
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

// 所有装配实例共享进程门禁；取消、失败和成功都保留冷却，不把失败登录排队或自动重试。
static LOGIN_SLOT: Semaphore = Semaphore::const_new(1);
static NEXT_LOGIN: Mutex<Option<Instant>> = Mutex::new(None);

/// 只将临时账号密码转换成指定用途的 Cookie，不保存密码，也不修改凭据启用状态。
pub struct CredentialAcquirer {
    key: Arc<DataKey>,
    client: Client,
    forum: EhentaiClient,
    endpoint: Url,
    login_page: Url,
    login_submit: Url,
}

impl CredentialAcquirer {
    /// 生产只连接固定论坛 HTTPS origin；不接受请求中的目标地址、代理或重定向策略。
    pub fn production(key: Arc<DataKey>) -> Result<Self, ApplicationError> {
        let endpoint = Url::parse(PRODUCTION_ENDPOINT).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "parse_endpoint",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        Self::from_endpoint(key, endpoint)
    }

    fn from_endpoint(key: Arc<DataKey>, endpoint: Url) -> Result<Self, ApplicationError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .referer(false)
            .https_only(endpoint.scheme() == "https")
            .tls_sslkeylogfile(false)
            .connection_verbose(false)
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "build_client",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
        let mut config = EhentaiClientConfig::production().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "configure_forum_client",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        config.endpoint = endpoint.clone();
        config.maximum_response_bytes = MAXIMUM_RESPONSE_BYTES;
        config.request_timeout = REQUEST_TIMEOUT;
        config.minimum_request_interval = REQUEST_INTERVAL;
        // 会话证明也复用禁止代理、重定向和协议级重试的客户端。
        let forum = EhentaiClient::from_config_and_client(config, client.clone()).map_err(
            |diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "construct_forum_client",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            },
        )?;
        let login_page =
            endpoint.join("index.php?act=Login&CODE=00").map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "resolve_login_page",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
        let login_submit =
            endpoint.join("index.php?act=Login&CODE=01").map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "resolve_login_submit",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
        Ok(Self { key, client, forum, endpoint, login_page, login_submit })
    }

    async fn login_once(
        &self,
        kind: AdminCredentialKind,
        login: AdminCredentialLogin,
    ) -> Result<AdminCredentialSecret, ApplicationError> {
        let response =
            self.client.get(self.login_page.clone()).send().await.map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "fetch_login_page",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
        let page = response::read(response, &self.endpoint, false).await?;
        let fields = form::parse(&page.body.0, &self.login_page, &self.login_submit)?;
        // 无 Cookie 的新登录页不能替调用者预先选择一个已认证账号。
        if page.cookies.has_authentication() {
            return Err(unavailable());
        }
        let (submit_url, encoded) = fields.encode(&login)?;
        drop(login);
        drop(page.body);
        let mut request = self
            .client
            .post(submit_url)
            .header(header::REFERER, self.login_page.as_str())
            .header(header::ORIGIN, self.endpoint.origin().ascii_serialization())
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(cookie) = page.cookies.header()? {
            request = request.header(header::COOKIE, cookie);
        }
        // 编码缓冲移交 reqwest 后不再复制；底层传输拥有的缓冲不能由此处保证清零。
        let request = request.body(encoded);
        tokio::time::sleep(REQUEST_INTERVAL).await;
        let response = request.send().await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "submit_login",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        let mut submitted = response::read(response, &self.endpoint, true).await?;
        drop(submitted.body);
        if !submitted.cookies.has_authentication() {
            // 没有认证 Cookie 也没有明确拒绝证据时，只能报告协议不可用，不能猜测密码错误。
            return Err(unavailable());
        }
        submitted.cookies.inherit_session(page.cookies);
        // 只用共享认证字段证明登录，避免论坛 session 偶然有效却掩盖不可用于游戏的认证字段。
        let authentication = submitted.cookies.authentication_secret()?;
        let cookie =
            EhentaiCookie::from_bytes(authentication.as_bytes()).map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "credential_login",
                    "validate_issued_cookie",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                unavailable()
            })?;
        tokio::time::sleep(REQUEST_INTERVAL).await;
        self.forum.check_session(&cookie).await.map_err(|error| {
            ApplicationError::AdminCredential(match error.category() {
                EhentaiErrorCategory::SessionExpired => AdminCredentialError::LoginRejected,
                EhentaiErrorCategory::Cloudflare => AdminCredentialError::LoginChallenge,
                EhentaiErrorCategory::RateLimited => AdminCredentialError::LoginBusy,
                _ => AdminCredentialError::LoginUnavailable,
            })
        })?;
        match kind {
            AdminCredentialKind::MailCookie => Ok(authentication),
            AdminCredentialKind::ForumCookie => submitted.cookies.into_secret(),
        }
    }
}

impl AdminCredentialAcquirer for CredentialAcquirer {
    fn login_digest(&self, login: &AdminCredentialLogin) -> Result<[u8; 32], ApplicationError> {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.key.material());
        let mut context = hmac::Context::with_key(&key);
        context.update(LOGIN_DIGEST_DOMAIN);
        // 长度前缀防止字段拼接歧义；不分配含密码的摘要输入副本，不生成裸密码散列。
        context.update(&(login.username().len() as u64).to_be_bytes());
        context.update(login.username().as_bytes());
        context.update(&(login.password().len() as u64).to_be_bytes());
        context.update(login.password());
        let mut result = [0; 32];
        result.copy_from_slice(context.sign().as_ref());
        Ok(result)
    }

    fn acquire(
        &self,
        kind: AdminCredentialKind,
        source: AdminCredentialImportSource,
    ) -> ApplicationFuture<'_, AdminCredentialSecret> {
        Box::pin(async move {
            match source {
                AdminCredentialImportSource::Cookie(secret) => cookies::normalize(kind, secret),
                AdminCredentialImportSource::ForumLogin(login) => {
                    let _permit = LoginPermit::acquire()?;
                    tokio::time::timeout(LOGIN_TIMEOUT, self.login_once(kind, login))
                        .await
                        .map_err(|diagnostic_cause| {
                            safe_diagnostics::warn(
                                "credential_login",
                                "login_timeout",
                                safe_diagnostics::project(&diagnostic_cause),
                                DiagnosticContext::default(),
                            );
                            unavailable()
                        })?
                }
            }
        })
    }
}

struct LoginPermit {
    _slot: SemaphorePermit<'static>,
}

impl LoginPermit {
    fn acquire() -> Result<Self, ApplicationError> {
        let slot = LOGIN_SLOT.try_acquire().map_err(|_| AdminCredentialError::LoginBusy)?;
        let next = NEXT_LOGIN.lock().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "credential_login",
                "lock_login_schedule",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            unavailable()
        })?;
        if next.is_some_and(|deadline| Instant::now() < deadline) {
            return Err(AdminCredentialError::LoginBusy.into());
        }
        Ok(Self { _slot: slot })
    }
}

impl Drop for LoginPermit {
    fn drop(&mut self) {
        if let Ok(mut next) = NEXT_LOGIN.lock() {
            *next = Some(Instant::now() + LOGIN_COOLDOWN);
        }
    }
}

/// 自有正文、隐藏字段及 Cookie 缓冲在正常退出、错误和取消时尽力清零。
struct SecretBuffer(Vec<u8>);

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

fn unavailable() -> ApplicationError {
    AdminCredentialError::LoginUnavailable.into()
}
