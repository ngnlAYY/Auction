//! 合并与分体部署共用的显式 PostgreSQL 目标校验，不连接数据库。

use super::{
    DeploymentDatabaseIdentity, DeploymentDatabaseIdentityError, deployment_database_identity,
    supported_postgres_query_key,
};

/// 拒绝隐式目标、未知参数和 search_path 覆盖后，使用驱动比较目标身份。
pub fn strict_deployment_database_identity(
    database_url: &str,
) -> Result<DeploymentDatabaseIdentity, DeploymentDatabaseIdentityError> {
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        let invalid = || {
            DeploymentDatabaseIdentityError(
                "PostgreSQL 部署连接必须显式指定目标，且不得包含未知参数或 options",
            )
        };
        let url = url::Url::parse(database_url).map_err(|_| invalid())?;
        if url.fragment().is_some() {
            return Err(invalid());
        }
        let mut host = url.host_str().is_some_and(|host| !host.is_empty());
        let mut port = url.port().is_some();
        let mut database = !url.path().trim_start_matches('/').is_empty();
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "host" | "hostaddr" => host = !value.is_empty(),
                "port" => port = !value.is_empty(),
                "dbname" => database = !value.is_empty(),
                _ if supported_postgres_query_key(&key, false) => {}
                _ => return Err(invalid()),
            }
        }
        if !host || !port || !database {
            return Err(invalid());
        }
    }
    deployment_database_identity(database_url)
}
