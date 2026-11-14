//! 来源适配器的装配与 HTTP client 构造；代理和自动重定向在这里统一关闭。

use std::sync::Arc;

use auction_application::mail::{
    MailDiagnosticRecorder, MailEquipmentDiagnosticContext, MailEquipmentDiagnosticSnapshot,
};
use reqwest::Client;
use tokio::{sync::Mutex, time::Instant};
use url::Url;

use super::super::config::{EquipmentSourceConfigurationError, EquipmentSourcePolicy};
use super::EquipmentSourceAdapter;
use crate::mooglemail::MoogleMailCredentialSet;

impl EquipmentSourceAdapter {
    /// 建立固定 endpoint 的来源适配器。
    /// 传入的 client 应由装配根使用 `build_equipment_http_client` 创建，从而关闭自动重定向
    /// 和环境代理；适配器仍会再次检查最终响应 URL。
    pub fn new(
        client: Client,
        endpoint: Url,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(EquipmentSourceConfigurationError::Endpoint);
        }
        policy.validate()?;
        Ok(Self {
            client,
            endpoint,
            policy,
            credentials: None,
            diagnostics: None,
            diagnostic_reporter: None,
            next_request_at: Mutex::new(Instant::now()),
        })
    }

    pub fn production(
        client: Client,
        policy: EquipmentSourcePolicy,
    ) -> Result<Self, EquipmentSourceConfigurationError> {
        let endpoint = Url::parse("https://hentaiverse.org")
            .map_err(|_| EquipmentSourceConfigurationError::Endpoint)?;
        Self::new(client, endpoint, policy)
    }

    /// 注入与 MoogleMail 共用的 world-aware 单 Cookie 集合。
    /// HentaiVerse 装备页要求登录；复用同一 Cookie 可以在远端明确返回登录页时通过窄端口
    /// 把数据库凭据标为失效。单 Cookie 失效后不自动切换，且 Cookie 不进入 URL、错误或日志。
    pub fn with_mooglemail_credentials(mut self, credentials: MoogleMailCredentialSet) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// 注入与邮件读取共用的诊断归档器；归档器只接收已脱敏关联字段和加密前的短缓冲。
    pub fn with_diagnostics(mut self, diagnostics: Arc<dyn MailDiagnosticRecorder>) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    /// 注入装配根拥有的安全日志出口；即使不留存原始响应，也可以记录失败摘要。
    pub fn with_diagnostic_reporter(
        mut self,
        reporter: impl Fn(&MailEquipmentDiagnosticContext, &MailEquipmentDiagnosticSnapshot)
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.diagnostic_reporter = Some(Arc::new(reporter));
        self
    }
}

/// 创建关闭代理和重定向的来源 HTTP client。
pub fn build_equipment_http_client() -> Result<Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build()
}
