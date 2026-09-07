//! AppConfig 的非敏感 getter 和敏感值最小暴露边界。

use super::super::*;
use super::model::AppConfig;

impl AppConfig {
    /// 返回数据库 URL。调用方不得把返回值写入日志、错误消息或 DTO。
    pub fn database_url(&self) -> Option<&str> {
        self.secrets.database_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回当前数据库档位下的有效 Worker 并发；SQLite Lite 的省略值固定为 1。
    pub fn worker_concurrency(&self) -> u16 {
        self.worker.concurrency.unwrap_or_else(|| {
            if self.database_url().is_some_and(database_url_is_sqlite) { 1 } else { 8 }
        })
    }

    /// 返回有效的 Worker 轮询间隔秒数；省略时沿用旧版 60 秒。
    pub fn worker_poll_interval_seconds(&self) -> u64 {
        self.worker.poll_interval_seconds.unwrap_or(60)
    }

    /// 返回 Redis URL。只有 `cache.backend = "redis"` 时才能使用。
    pub fn redis_url(&self) -> Option<&str> {
        self.secrets.redis_url.as_ref().map(SecretString::expose_secret)
    }

    /// 返回会话签名密钥。调用方不得输出、持久化副本或传给浏览器。
    pub fn session_signing_key(&self) -> Option<&str> {
        self.secrets.session_signing_key.as_ref().map(SecretString::expose_secret)
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

    /// 返回装备 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn equipment_data_key_id(&self) -> Option<&str> {
        self.secrets.equipment_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回装备 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn equipment_data_key(&self) -> Option<&str> {
        self.secrets.equipment_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断装备 key 加密配置是否成对提供。
    pub fn equipment_data_key_configured(&self) -> bool {
        self.equipment_data_key_id().is_some() && self.equipment_data_key().is_some()
    }

    /// 返回用户登录 key 加密使用的数据密钥标识；只允许密钥装配根读取。
    pub fn user_login_data_key_id(&self) -> Option<&str> {
        self.secrets.user_login_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回用户登录 key 加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn user_login_data_key(&self) -> Option<&str> {
        self.secrets.user_login_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断用户登录 key 加密配置是否成对提供。
    pub fn user_login_data_key_configured(&self) -> bool {
        self.user_login_data_key_id().is_some() && self.user_login_data_key().is_some()
    }

    /// 返回外部凭据加密使用的数据密钥标识；只允许凭据装配根读取。
    pub fn credential_data_key_id(&self) -> Option<&str> {
        self.secrets.credential_data_key_id.as_ref().map(SecretString::expose_secret)
    }

    /// 返回外部凭据加密使用的数据密钥材料；调用方不得输出或持久化副本。
    pub fn credential_data_key(&self) -> Option<&str> {
        self.secrets.credential_data_key.as_ref().map(SecretString::expose_secret)
    }

    /// 判断外部凭据加密配置是否成对提供。
    pub fn credential_data_key_configured(&self) -> bool {
        self.credential_data_key_id().is_some() && self.credential_data_key().is_some()
    }

    /// 判断 Admin 登录凭据是否成对配置，不暴露任何凭据内容。
    pub fn admin_credentials_configured(&self) -> bool {
        self.admin_actor().is_some() && self.admin_secret().is_some()
    }

    /// 返回 Admin dashboard 连接 Worker control 使用的 socket 路径。
    ///
    /// runtime 文档省略时使用应用模型的默认路径；该 getter 不读取环境变量，也不暴露敏感值。
    pub fn worker_control_socket_path(&self) -> &str {
        self.admin_runtime
            .as_ref()
            .map(|document| document.worker_control.socket_path.as_str())
            .unwrap_or("/run/hv-auction/worker/control.sock")
    }

    /// 返回 Admin 当前生效的 Unix listener socket；TCP 模式和其它角色返回 `None`。
    ///
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
