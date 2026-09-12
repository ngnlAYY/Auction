//! 部署数据库身份预检；禁止目标依赖操作者或 systemd 的不同环境。

use anyhow::Context;
use auction_storage::{DeploymentDatabaseIdentity, deployment_database_identity};

pub(super) fn database_identity(database_url: &str) -> anyhow::Result<DeploymentDatabaseIdentity> {
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        let url = url::Url::parse(database_url)
            .map_err(|_| anyhow::anyhow!("PostgreSQL 部署连接参数无效"))?;
        if url.fragment().is_some() {
            anyhow::bail!("PostgreSQL 部署连接不能包含 fragment");
        }
        let mut explicit_host = url.host_str().is_some_and(|host| !host.is_empty());
        let mut explicit_port = url.port().is_some();
        let mut explicit_database = !url.path().trim_start_matches('/').is_empty();
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "host" | "hostaddr" => explicit_host = !value.is_empty(),
                "port" => explicit_port = !value.is_empty(),
                "dbname" => explicit_database = !value.is_empty(),
                "sslmode"
                | "ssl-mode"
                | "sslrootcert"
                | "ssl-root-cert"
                | "ssl-ca"
                | "sslcert"
                | "ssl-cert"
                | "sslkey"
                | "ssl-key"
                | "statement-cache-capacity"
                | "user"
                | "password"
                | "application_name" => {}
                // options 可改变 search_path，让同一个数据库内的角色访问不同 schema。
                "options" => anyhow::bail!("部署连接不能通过 options 改变服务端会话语义"),
                _ => anyhow::bail!("PostgreSQL 部署连接包含不支持的参数"),
            }
        }
        if !explicit_host || !explicit_port || !explicit_database {
            anyhow::bail!(
                "PostgreSQL 部署连接必须显式指定 host、port 和数据库，不能依赖环境默认值"
            );
        }
    }
    deployment_database_identity(database_url).context("确认部署数据库目标失败")
}
