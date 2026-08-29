//! PostgreSQL bootstrap 的集群锁、权限预检、CREATE DATABASE 和后置验证。

use sqlx::{Connection, PgConnection, Row};

use super::{
    model::{PostgresBootstrapResult, PostgresDatabaseBootstrapError},
    sql,
};
use crate::database::rebuild::DATABASE_ADVISORY_LOCK_PREFIX;

/// 持有集群级 advisory lock 的 PostgreSQL 初始数据库 bootstrap 会话。
///
/// 维护连接从构造到 Drop 始终指向 `postgres` 数据库，因此锁生命周期覆盖 catalog 检查、
/// 可能的 CREATE DATABASE 以及应用连接的后置验证。该类型没有迁移和 DROP 入口。
pub struct PostgresDatabaseBootstrap {
    maintenance_connection: PgConnection,
    database_name: String,
    owner: String,
}

impl PostgresDatabaseBootstrap {
    /// 连接维护库并取得本项目数据库级 advisory lock。
    pub async fn connect(
        maintenance_database_url: &str,
        database_name: &str,
        owner: &str,
    ) -> Result<Self, PostgresDatabaseBootstrapError> {
        sql::validate_database_name(database_name)?;
        sql::validate_owner(owner)?;
        let mut maintenance_connection = PgConnection::connect(maintenance_database_url)
            .await
            .map_err(|_| PostgresDatabaseBootstrapError::MaintenanceConnection)?;
        configure_session(&mut maintenance_connection).await?;

        let maintenance_database =
            sqlx::query_scalar::<_, String>("SELECT current_database()::text")
                .fetch_one(&mut maintenance_connection)
                .await
                .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery {
                    stage: "maintenance_database",
                })?;
        if maintenance_database != "postgres" {
            return Err(PostgresDatabaseBootstrapError::WrongMaintenanceDatabase);
        }

        let lock_acquired = sqlx::query_scalar::<_, bool>(
            "SELECT pg_try_advisory_lock(hashtextextended($1 || $2, 0))",
        )
        .bind(DATABASE_ADVISORY_LOCK_PREFIX)
        .bind(database_name)
        .fetch_one(&mut maintenance_connection)
        .await
        .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "advisory_lock" })?;
        if !lock_acquired {
            return Err(PostgresDatabaseBootstrapError::AdvisoryLockBusy);
        }

        Ok(Self {
            maintenance_connection,
            database_name: database_name.to_owned(),
            owner: owner.to_owned(),
        })
    }

    /// 在同一 advisory lock 内执行非破坏性 bootstrap，并验证应用 URL 可以连接新库。
    pub async fn create_or_verify(
        &mut self,
        application_database_url: &str,
    ) -> Result<PostgresBootstrapResult, PostgresDatabaseBootstrapError> {
        let cluster = read_cluster_privileges(&mut self.maintenance_connection).await?;
        if cluster.in_recovery || cluster.default_transaction_read_only {
            return Err(PostgresDatabaseBootstrapError::ReadOnlyPrimary);
        }

        // 已存在且身份完全匹配的数据库是纯只读幂等结果，不要求维护角色额外持有 CREATEDB
        // 或应用 owner 的成员关系；这些高权限只在本次确实需要 CREATE DATABASE 时校验。
        if let Some(catalog_owner) =
            database_owner(&mut self.maintenance_connection, &self.database_name).await?
        {
            if catalog_owner != self.owner {
                return Err(PostgresDatabaseBootstrapError::CreatedDatabaseVerification);
            }
            verify_application_connection(
                application_database_url,
                &self.database_name,
                &self.owner,
            )
            .await?;
            return Ok(PostgresBootstrapResult::AlreadyExists {
                database_name: self.database_name.clone(),
                owner: self.owner.clone(),
            });
        }

        if !cluster.is_superuser && !cluster.can_create_database {
            return Err(PostgresDatabaseBootstrapError::MissingCreateDatabasePrivilege);
        }

        if !role_exists(&mut self.maintenance_connection, &self.owner).await? {
            return Err(PostgresDatabaseBootstrapError::OwnerRoleMissing);
        }
        if !cluster.is_superuser
            && !can_assign_owner(&mut self.maintenance_connection, &self.owner).await?
        {
            return Err(PostgresDatabaseBootstrapError::CannotAssignOwner);
        }

        let statement = sql::create_database_sql(&self.database_name, &self.owner);
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut self.maintenance_connection)
            .await
            .map_err(|_| PostgresDatabaseBootstrapError::CreateDatabase)?;

        let catalog_owner = database_owner(&mut self.maintenance_connection, &self.database_name)
            .await?
            .ok_or(PostgresDatabaseBootstrapError::CreatedDatabaseVerification)?;
        if catalog_owner != self.owner {
            return Err(PostgresDatabaseBootstrapError::CreatedDatabaseVerification);
        }

        verify_application_connection(application_database_url, &self.database_name, &self.owner)
            .await?;

        Ok(PostgresBootstrapResult::Created {
            database_name: self.database_name.clone(),
            owner: self.owner.clone(),
        })
    }
}

struct ClusterPrivileges {
    in_recovery: bool,
    default_transaction_read_only: bool,
    is_superuser: bool,
    can_create_database: bool,
}

async fn configure_session(
    connection: &mut PgConnection,
) -> Result<(), PostgresDatabaseBootstrapError> {
    for (stage, statement) in [
        ("application_name", "SET application_name = 'auctionctl/postgres-bootstrap'"),
        ("search_path", "SET search_path = pg_catalog"),
        ("standard_conforming_strings", "SET standard_conforming_strings = on"),
        ("statement_timeout", "SET statement_timeout = '30s'"),
        ("lock_timeout", "SET lock_timeout = '5s'"),
    ] {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage })?;
    }
    Ok(())
}

async fn read_cluster_privileges(
    connection: &mut PgConnection,
) -> Result<ClusterPrivileges, PostgresDatabaseBootstrapError> {
    let row = sqlx::query(
        "SELECT pg_is_in_recovery() AS in_recovery,
                current_setting('default_transaction_read_only') = 'on'
                    AS default_transaction_read_only,
                role.rolsuper AS is_superuser,
                role.rolcreatedb AS can_create_database
         FROM pg_catalog.pg_roles AS role
         WHERE role.rolname = current_user",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "cluster_privileges" })?;
    Ok(ClusterPrivileges {
        in_recovery: row.try_get("in_recovery").map_err(|_| {
            PostgresDatabaseBootstrapError::CatalogQuery { stage: "cluster_privileges" }
        })?,
        default_transaction_read_only: row.try_get("default_transaction_read_only").map_err(
            |_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "cluster_privileges" },
        )?,
        is_superuser: row.try_get("is_superuser").map_err(|_| {
            PostgresDatabaseBootstrapError::CatalogQuery { stage: "cluster_privileges" }
        })?,
        can_create_database: row.try_get("can_create_database").map_err(|_| {
            PostgresDatabaseBootstrapError::CatalogQuery { stage: "cluster_privileges" }
        })?,
    })
}

async fn role_exists(
    connection: &mut PgConnection,
    owner: &str,
) -> Result<bool, PostgresDatabaseBootstrapError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = $1)")
        .bind(owner)
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "owner_role" })
}

async fn can_assign_owner(
    connection: &mut PgConnection,
    owner: &str,
) -> Result<bool, PostgresDatabaseBootstrapError> {
    sqlx::query_scalar(
        "SELECT current_user = $1
                OR pg_has_role(current_user, $1, 'MEMBER')",
    )
    .bind(owner)
    .fetch_one(&mut *connection)
    .await
    .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "owner_membership" })
}

async fn database_owner(
    connection: &mut PgConnection,
    database_name: &str,
) -> Result<Option<String>, PostgresDatabaseBootstrapError> {
    sqlx::query_scalar(
        "SELECT pg_catalog.pg_get_userbyid(datdba)::text
         FROM pg_catalog.pg_database
         WHERE datname = $1",
    )
    .bind(database_name)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| PostgresDatabaseBootstrapError::CatalogQuery { stage: "database_owner" })
}

async fn verify_application_connection(
    application_database_url: &str,
    database_name: &str,
    owner: &str,
) -> Result<(), PostgresDatabaseBootstrapError> {
    let mut connection = PgConnection::connect(application_database_url)
        .await
        .map_err(|_| PostgresDatabaseBootstrapError::ApplicationConnection)?;
    let row = sqlx::query("SELECT current_database()::text, current_user::text")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| PostgresDatabaseBootstrapError::ApplicationConnection)?;
    let current_database: String =
        row.try_get(0).map_err(|_| PostgresDatabaseBootstrapError::ApplicationIdentityMismatch)?;
    let current_user: String =
        row.try_get(1).map_err(|_| PostgresDatabaseBootstrapError::ApplicationIdentityMismatch)?;
    if current_database != database_name || current_user != owner {
        return Err(PostgresDatabaseBootstrapError::ApplicationIdentityMismatch);
    }
    connection.close().await.map_err(|_| PostgresDatabaseBootstrapError::ApplicationConnection)
}
