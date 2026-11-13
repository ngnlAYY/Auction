//! 从敏感连接串提取非秘密的目标身份（主机、端口、库名、账号）；口令只在 libpq 适配器中短暂使用。

use anyhow::Context;

/// PostgreSQL 连接目标的非秘密部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabaseTarget {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) username: String,
}

impl DatabaseTarget {
    /// 复用实际 SQLx 解析选项，避免 CLI 对 host/hostaddr 顺序和 socket 优先级另作解释。
    pub(crate) fn parse(url: &str) -> anyhow::Result<Self> {
        let options = auction_storage::postgres_connect_options(url)
            .map_err(|_| anyhow::anyhow!("数据库连接参数无效"))?;
        let host = if let Some(socket) = options.get_socket() {
            socket.to_str().context("数据库 socket 路径不是 UTF-8")?.to_owned()
        } else {
            options.get_host().to_owned()
        };
        let port = options.get_port();
        let database = options.get_database().unwrap_or_default().to_owned();
        let username = options.get_username().to_owned();
        if host.is_empty()
            || port == 0
            || database.is_empty()
            || database.contains('/')
            || [&host, &database, &username].iter().any(|value| value.chars().any(char::is_control))
        {
            anyhow::bail!("数据库连接参数无效");
        }
        Ok(Self { host, port, database, username })
    }

    /// 人工确认的库名必须与配置目标逐字一致。
    pub(crate) fn require_database(&self, confirmed: &str) -> anyhow::Result<()> {
        if self.database != confirmed {
            anyhow::bail!("--database 与 Worker 配置中的数据库名不一致");
        }
        Ok(())
    }

    /// 维护连接必须指向同一 PostgreSQL 实例；Unix socket 路径区分大小写。
    pub(crate) fn require_same_endpoint(&self, other: &Self) -> anyhow::Result<()> {
        let same_host = if self.host.starts_with('/') || other.host.starts_with('/') {
            self.host == other.host
        } else if let (Ok(left), Ok(right)) =
            (self.host.parse::<std::net::IpAddr>(), other.host.parse::<std::net::IpAddr>())
        {
            left == right
        } else {
            self.host.eq_ignore_ascii_case(&other.host)
        };
        if !same_host || self.port != other.port {
            anyhow::bail!("维护凭据与应用数据库必须指向同一 PostgreSQL 主机与端口");
        }
        Ok(())
    }
}

/// 解码 URI 百分号转义；非法转义、非 UTF-8 或控制字符一律拒绝，错误不回显原文。
pub(crate) fn decode_component(value: &str) -> anyhow::Result<String> {
    let decoded = auction_application::core::strict_percent_decode(value)
        .context("数据库连接参数含无效的百分号转义或不是 UTF-8")?;
    if decoded.chars().any(char::is_control) {
        anyhow::bail!("数据库连接参数含控制字符");
    }
    Ok(decoded.into_owned())
}
