//! 父进程持有已迁移的夹具，子进程只克隆自己的库，父进程负责回收子进程的资源。

use std::env;
use std::time::Duration;

use auction_storage::{DatabaseConnectionProfile, DatabaseStore};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use url::Url;
use uuid::Uuid;

use super::{TEST_TIMEOUT, run_test, test_command};
use crate::support::TestDatabase;

mod tests;

const AUCTION_TEST_DATABASE_URL: &str = "AUCTION_TEST_DATABASE_URL";
const ADMIN_URL: &str = "AUCTION_AUDIT_TEST_ADMIN_URL";
const RUNTIME_URL: &str = "AUCTION_AUDIT_TEST_RUNTIME_URL";
const TEMPLATE: &str = "AUCTION_AUDIT_TEST_TEMPLATE";
const PREFIX: &str = "AUCTION_AUDIT_TEST_PREFIX";
const PREFIX_START: &str = "auction_audit_";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

/// 保持共享集群和克隆源存活，直到子进程退出并清理完毕。
pub(super) struct ParentFixture {
    _database: TestDatabase,
    admin_url: Url,
    template: String,
}

impl ParentFixture {
    /// 复用公共夹具完成运行角色初始化与迁移，在父进程保留一个无连接的克隆源。
    pub(super) async fn prepare() -> Result<Self, String> {
        let database = TestDatabase::create().await.map_err(|error| {
            // 保留中继依赖的迁移诊断字面量，同时避免将维护 URL 的错误链带入诊断。
            if error.contains("模板库执行迁移失败") {
                "模板库执行迁移失败"
            } else {
                "初始化审计子测试的父夹具失败"
            }
            .to_owned()
        })?;
        let maintenance =
            database.maintenance_store().await.map_err(|_error| "连接父夹具失败".to_owned())?;
        let options = maintenance.postgres_pool().connect_options();
        let template = options.get_database().map(str::to_owned);
        maintenance.postgres_pool().close().await;
        let template = template.ok_or_else(|| "父夹具缺少克隆源".to_owned())?;
        let admin_url = match env::var(AUCTION_TEST_DATABASE_URL) {
            Ok(url) if !url.trim().is_empty() => Url::parse(url.trim()),
            // 公共夹具自行启动的集群固定使用回环维护 URL。
            _ => Url::parse(&format!(
                "postgresql://postgres@127.0.0.1:{}/postgres",
                options.get_port()
            )),
        }
        .map_err(|_error| "解析父夹具维护地址失败".to_owned())?;
        Ok(Self { _database: database, admin_url, template })
    }

    /// 生成一次性名字，初始化子测试使用的最小权限登录，再从父进程启动和回收子进程。
    pub(super) async fn run(&self, child: &str) -> Result<(), String> {
        self.run_with_prefix(child, &new_prefix()).await
    }

    async fn run_with_prefix(&self, child: &str, prefix: &str) -> Result<(), String> {
        let mut command = test_command(child)?;
        let role = format!("{prefix}runtime");
        let password = Uuid::new_v4().simple().to_string();
        let mut runtime_url = self.admin_url.clone();
        runtime_url.set_username(&role).map_err(|()| "设置临时角色失败".to_owned())?;
        runtime_url.set_password(Some(&password)).map_err(|()| "设置临时凭据失败".to_owned())?;

        let mut connection = control_connection(&self.admin_url, prefix).await?;
        let created = execute(
            &mut connection,
            format!(
                "CREATE ROLE {role} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS \
                 PASSWORD '{password}' IN ROLE auction_runtime"
            ),
            "创建子测试运行角色失败",
        )
        .await;
        drop(connection);
        if let Err(error) = created {
            return match self.cleanup(prefix).await {
                Ok(()) => Err(error),
                Err(cleanup) => Err(format!("{error}；{cleanup}")),
            };
        }

        command
            .env(ADMIN_URL, self.admin_url.as_str())
            .env(RUNTIME_URL, runtime_url.as_str())
            .env(TEMPLATE, &self.template)
            .env(PREFIX, prefix);
        run_test(command, child, TEST_TIMEOUT, self.cleanup(prefix)).await
    }

    async fn cleanup(&self, prefix: &str) -> Result<(), String> {
        tokio::time::timeout(CONTROL_TIMEOUT, self.drop_resources(prefix))
            .await
            .map_err(|_error| "清理子测试资源超时".to_owned())?
    }

    async fn drop_resources(&self, prefix: &str) -> Result<(), String> {
        let mut connection = control_connection(&self.admin_url, prefix).await?;
        // 先等待被杀子进程的建库语句退出，再列举资源，避免清理早于 CREATE DATABASE 提交。
        lock_prefix(&mut connection, prefix).await?;
        let databases: Vec<String> = sqlx::query_scalar(
            "SELECT datname::text FROM pg_database WHERE starts_with(datname::text, $1)",
        )
        .bind(prefix)
        .fetch_all(&mut connection)
        .await
        .map_err(|error| sql_error("列举子测试数据库失败", &error))?;
        for database in databases {
            execute(
                &mut connection,
                format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(&database)),
                "清理子测试数据库失败",
            )
            .await?;
        }
        let roles: Vec<String> = sqlx::query_scalar(
            "SELECT rolname::text FROM pg_roles WHERE starts_with(rolname::text, $1)",
        )
        .bind(prefix)
        .fetch_all(&mut connection)
        .await
        .map_err(|error| sql_error("列举子测试角色失败", &error))?;
        for role in roles {
            execute(
                &mut connection,
                format!("DROP ROLE IF EXISTS {}", quote_ident(&role)),
                "清理子测试角色失败",
            )
            .await?;
        }
        Ok(())
    }
}

/// 仅接受父测试提供的夹具。返回运行账号、维护账号，且只克隆自己的临时库。
pub(super) async fn open_child_database() -> Result<(DatabaseStore, DatabaseStore), String> {
    let admin_url = child_url(ADMIN_URL)?;
    let mut runtime_url = child_url(RUNTIME_URL)?;
    let template = env::var(TEMPLATE).map_err(|_error| "缺少父夹具克隆源".to_owned())?;
    let prefix = env::var(PREFIX).map_err(|_error| "缺少父测试分配的前缀".to_owned())?;
    if !prefix.strip_prefix(PREFIX_START).and_then(|suffix| suffix.strip_suffix('_')).is_some_and(
        |token| token.len() == 32 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
    ) {
        return Err("父测试分配的前缀无效".to_owned());
    }
    let database = format!("{prefix}db");
    let mut connection = control_connection(&admin_url, &prefix).await?;
    lock_prefix(&mut connection, &prefix).await?;
    execute(
        &mut connection,
        format!("CREATE DATABASE {} TEMPLATE {}", quote_ident(&database), quote_ident(&template)),
        "克隆审计子测试数据库失败",
    )
    .await?;
    drop(connection);

    let mut maintenance_url = admin_url;
    maintenance_url.set_path(&database);
    runtime_url.set_path(&database);
    let store = DatabaseStore::connect(runtime_url.as_str(), DatabaseConnectionProfile::legacy())
        .await
        .map_err(|_error| "连接审计子测试运行账号失败".to_owned())?;
    let maintenance =
        DatabaseStore::connect(maintenance_url.as_str(), DatabaseConnectionProfile::legacy())
            .await
            .map_err(|_error| "连接审计子测试维护账号失败".to_owned())?;
    Ok((store, maintenance))
}

fn new_prefix() -> String {
    format!("{PREFIX_START}{}_", Uuid::new_v4().simple())
}

fn child_url(name: &str) -> Result<Url, String> {
    let value = env::var(name).map_err(|_error| "审计子测试必须由父测试启动".to_owned())?;
    Url::parse(&value).map_err(|_error| "父夹具地址无效".to_owned())
}

async fn control_connection(admin_url: &Url, prefix: &str) -> Result<PgConnection, String> {
    let options = admin_url
        .as_str()
        .parse::<PgConnectOptions>()
        .map_err(|_error| "解析维护连接选项失败".to_owned())?
        .application_name(prefix);
    let mut connection =
        tokio::time::timeout(CONTROL_TIMEOUT, PgConnection::connect_with(&options))
            .await
            .map_err(|_error| "连接维护库超时".to_owned())?
            .map_err(|error| sql_error("连接维护库失败", &error))?;
    sqlx::query("SET statement_timeout = '25s'")
        .execute(&mut connection)
        .await
        .map_err(|error| sql_error("设置维护连接时限失败", &error))?;
    Ok(connection)
}

async fn lock_prefix(connection: &mut PgConnection, prefix: &str) -> Result<(), String> {
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(prefix)
        .execute(connection)
        .await
        .map_err(|error| sql_error("取得子测试资源锁失败", &error))?;
    Ok(())
}

async fn execute(
    connection: &mut PgConnection,
    statement: String,
    label: &str,
) -> Result<(), String> {
    sqlx::raw_sql(AssertSqlSafe(statement))
        .execute(connection)
        .await
        .map_err(|error| sql_error(label, &error))?;
    Ok(())
}

fn sql_error(label: &str, error: &sqlx::Error) -> String {
    match error.as_database_error().and_then(sqlx::error::DatabaseError::code) {
        Some(code) => format!("{label}（SQLSTATE {code}）"),
        None => label.to_owned(),
    }
}

fn quote_ident(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
