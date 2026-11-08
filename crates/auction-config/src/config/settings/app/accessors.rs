//! AppConfig 的非敏感 getter 和敏感值最小暴露边界。

use auction_domain::DataKeyDomain;

use super::super::*;
use super::model::AppConfig;

impl AppConfig {
    /// R2 access key 只交给备份传输适配器，不得记录或持久化副本。
    pub fn backup_r2_access_key_id(&self) -> Option<&str> {
        self.secrets.backup_r2_access_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// R2 secret 只交给备份传输适配器，不得记录或持久化副本。
    pub fn backup_r2_secret_access_key(&self) -> Option<&str> {
        self.secrets.backup_r2_secret_access_key.as_ref().map(SecretString::expose_secret)
    }

    /// 返回成功打卡地址；只能交给备份传输适配器，不得记录或持久化。
    pub fn backup_heartbeat_success_url(&self) -> Option<&str> {
        self.secrets.backup_heartbeat_success_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回失败打卡地址；只能交给备份传输适配器，不得记录或持久化。
    pub fn backup_heartbeat_failure_url(&self) -> Option<&str> {
        self.secrets.backup_heartbeat_failure_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 Worker 外部存活心跳地址；只能交给心跳循环，不得记录或持久化。
    pub fn worker_heartbeat_url(&self) -> Option<&str> {
        self.secrets.worker_heartbeat_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回运维告警 webhook 地址；只交给 webhook 适配器，不记录或持久化。
    pub fn operations_alert_webhook_url(&self) -> Option<&str> {
        self.secrets.operations_alert_webhook_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回数据库 URL。调用方不得把返回值写入日志、错误消息或 DTO。
    pub fn database_url(&self) -> Option<&str> {
        self.secrets.database_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回有效 Worker 并发；省略时使用 8。
    pub fn worker_concurrency(&self) -> u16 {
        self.worker.concurrency.unwrap_or(8)
    }

    /// 返回有效的 Worker 轮询间隔秒数；省略时沿用旧版 60 秒。
    pub fn worker_poll_interval_seconds(&self) -> u64 {
        self.worker.poll_interval_seconds.unwrap_or(60)
    }

    /// 返回 E-Hentai Cookie。只允许外部集成适配器读取。
    pub fn ehentai_cookie(&self) -> Option<&str> {
        self.secrets.ehentai_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 MoogleMail Cookie。只允许外部集成适配器读取。
    pub fn mooglemail_cookie(&self) -> Option<&str> {
        self.secrets.mooglemail_cookie.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前角色的 OTLP headers；调用方只允许把它交给 tracing exporter，禁止记录或持久化。
    pub fn tracing_headers(&self) -> Option<&str> {
        match self.role {
            Role::Public => self.secrets.public_otlp_headers.as_ref(),
            Role::Admin => self.secrets.admin_otlp_headers.as_ref(),
            Role::Worker => self.secrets.worker_otlp_headers.as_ref(),
        }
        .map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 actor；只允许 Admin 装配根读取。
    pub fn admin_actor(&self) -> Option<&str> {
        self.secrets.admin_actor.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 Admin 登录 secret；调用方不得输出、持久化副本或传给浏览器。
    pub fn admin_secret(&self) -> Option<&str> {
        self.secrets.admin_secret.as_ref().map(SecretString::expose_secret)
    }

    /// 部署检查使用的静态安全警告；仅返回固定文案，不暴露秘密或监听路径。
    pub fn admin_security_warnings(&self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if self.role != Role::Admin {
            return warnings;
        }
        if self.admin_secret().is_some_and(|secret| secret.len() < 32) {
            warnings.push("admin_secret 短于 32 字节；请轮换为密码管理器生成的至少 32 字节随机值");
        }
        let behind_local_proxy = self.listen_addr.is_none_or(|address| address.ip().is_loopback());
        if self.http_security.cookie_secure
            && behind_local_proxy
            && self.http_security.trusted_proxy_cidrs.is_empty()
            && !self.http_security.trusted_unix_proxy
        {
            warnings.push("Admin 生产形态未配置信任代理；请核对 trusted_proxy_cidrs/trusted_unix_proxy 与 CDN 的真实客户端地址链");
        }
        warnings
    }

    /// 返回装备 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn equipment_data_key_id(&self) -> Option<&str> {
        self.secrets.equipment_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn equipment_data_key(&self) -> Option<&str> {
        self.secrets.equipment_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 返回用户登录 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn user_login_data_key_id(&self) -> Option<&str> {
        self.secrets.user_login_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回用户登录 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn user_login_data_key(&self) -> Option<&str> {
        self.secrets.user_login_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 返回外部凭据加密使用的数据密钥标识；只允许凭据装配根读取。
    pub fn credential_data_key_id(&self) -> Option<&str> {
        self.secrets.credential_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回外部凭据加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn credential_data_key(&self) -> Option<&str> {
        self.secrets.credential_data_key.as_ref().map(SecretString::expose_secret)
    }

    pub fn credential_data_key_configured(&self) -> bool {
        self.credential_data_key_id().is_some() && self.credential_data_key().is_some()
    }

    /// 返回 MoogleMail 诊断归档的独立父密钥标识；只允许诊断装配根读取。
    pub fn mail_diagnostics_data_key_id(&self) -> Option<&str> {
        self.secrets.mail_diagnostics_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回 MoogleMail 诊断归档的独立父密钥材料；调用方不得输出或持久化副本。
    pub fn mail_diagnostics_data_key(&self) -> Option<&str> {
        self.secrets.mail_diagnostics_data_key.as_ref().map(SecretString::expose_secret)
    }

    pub fn mail_diagnostics_data_key_configured(&self) -> bool {
        self.mail_diagnostics_data_key_id().is_some() && self.mail_diagnostics_data_key().is_some()
    }

    /// 返回指定域当前写入密钥的标识与材料；未配置时返回 `None`。
    pub fn data_key_current(&self, domain: DataKeyDomain) -> Option<(&str, &str)> {
        let (id, material) = match domain {
            DataKeyDomain::Equipment => {
                (&self.secrets.equipment_data_key_id, &self.secrets.equipment_data_key)
            }
            DataKeyDomain::UserLogin => {
                (&self.secrets.user_login_data_key_id, &self.secrets.user_login_data_key)
            }
            DataKeyDomain::Credential => {
                (&self.secrets.credential_data_key_id, &self.secrets.credential_data_key)
            }
            DataKeyDomain::MailDiagnostics => (
                &self.secrets.mail_diagnostics_data_key_id,
                &self.secrets.mail_diagnostics_data_key,
            ),
        };
        Some((id.as_ref()?.expose_secret(), material.as_ref()?.expose_secret()))
    }

    /// 返回指定域保留解密的旧密钥；保留期由配置计算。
    pub fn data_key_previous(&self, domain: DataKeyDomain) -> &[RetiredDataKey] {
        match domain {
            DataKeyDomain::Equipment => &self.secrets.equipment_previous_data_keys,
            DataKeyDomain::UserLogin => &self.secrets.user_login_previous_data_keys,
            DataKeyDomain::Credential => &self.secrets.credential_previous_data_keys,
            DataKeyDomain::MailDiagnostics => &self.secrets.mail_diagnostics_previous_data_keys,
        }
    }

    /// 判断 Admin 登录凭据是否成对配置，不暴露任何凭据内容。
    pub fn admin_credentials_configured(&self) -> bool {
        self.admin_actor().is_some() && self.admin_secret().is_some()
    }

    /// 返回 Admin dashboard 连接 Worker control 使用的 socket 路径。
    /// runtime 文档省略时使用应用模型的默认路径；该 getter 不读取环境变量，也不暴露敏感值。
    pub fn worker_control_socket_path(&self) -> &str {
        self.admin_runtime
            .as_ref()
            .map(|document| document.worker_control.socket_path.as_str())
            .unwrap_or("/run/hv-auction/worker/control.sock")
    }

    /// 返回 Admin 当前生效的 Unix listener socket；TCP 模式和其它角色返回 `None`。
    /// 该 getter 只暴露非敏感监听路径，供部署预检和 readiness 探针与 Admin 装配根使用同一
    /// 权威配置，避免部署控制器仍探测已经被 runtime 覆盖掉的 TCP 地址。
    pub fn admin_unix_listener_socket_path(&self) -> Option<&str> {
        (self.role == Role::Admin).then_some(self.listener.as_ref()).flatten().and_then(
            |listener| {
                (listener.mode == HttpListenMode::Unix).then_some(listener.socket_path.as_str())
            },
        )
    }
}
