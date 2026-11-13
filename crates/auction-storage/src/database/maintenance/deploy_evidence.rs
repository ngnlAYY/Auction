//! 部署安全门禁的只读证据：已越过 sending 围栏的在途外部发送，以及迁移兼容后缀。
//! 两者都在有界只读快照内读取，不领取任务、不加行锁、不改写任何事实。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use thiserror::Error;
use uuid::Uuid;

use crate::database::{DatabaseStore, set_read_only_limits};

/// 部署前在途发送投影；只含内部标识与时间，不含收件人、正文或凭据。
#[derive(Debug, Serialize)]
pub struct PostgresInFlightSends {
    pub checked_at: DateTime<Utc>,
    pub mail_sends: Vec<InFlightMailSend>,
    pub pm_sends: Vec<InFlightPmSend>,
}

impl PostgresInFlightSends {
    pub fn is_empty(&self) -> bool {
        self.mail_sends.is_empty() && self.pm_sends.is_empty()
    }

    pub fn len(&self) -> usize {
        self.mail_sends.len() + self.pm_sends.len()
    }
}

/// 已越过发送围栏、租约仍然有效的邮件投递。
#[derive(Debug, Serialize)]
pub struct InFlightMailSend {
    pub delivery_id: Uuid,
    pub sending_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
}

/// 已越过发送围栏、租约仍然有效的 PM 投递。
#[derive(Debug, Serialize)]
pub struct InFlightPmSend {
    pub notification_id: Uuid,
    pub sending_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
}

/// 迁移兼容后缀证据；`level` 缺失表示该版本没有登记，绝不能当作兼容。
#[derive(Debug, Serialize)]
pub struct PostgresMigrationSuffix {
    pub current_version: i64,
    pub entries: Vec<MigrationCompatibilityEntry>,
}

#[derive(Debug, Serialize)]
pub struct MigrationCompatibilityEntry {
    pub version: i64,
    pub success: bool,
    pub level: Option<String>,
}

/// 只报告固定阶段、SQLx 变体名与校验后的 SQLSTATE，不携带原始错误或查询文本。
#[derive(Debug, Error)]
#[error(
    "读取 PostgreSQL 部署门禁证据失败（stage={stage}, sqlx_kind={sqlx_kind}, sqlstate={sqlstate}）"
)]
pub struct DeployEvidenceError {
    stage: &'static str,
    sqlx_kind: &'static str,
    sqlstate: String,
}

impl DatabaseStore {
    /// 读取已越过 sending 围栏且租约未过期的邮件与 PM 投递；已过期租约只代表结果未知，不在此集合。
    pub async fn collect_in_flight_sends(
        &self,
    ) -> Result<PostgresInFlightSends, DeployEvidenceError> {
        let mut transaction = self
            .pool()
            .begin_read_snapshot()
            .await
            .map_err(|error| failure("begin_read_only_transaction", &error))?;
        set_read_only_limits(&mut transaction)
            .await
            .map_err(|(stage, error)| failure(stage, &error))?;

        let clock = sqlx::query("SELECT statement_timestamp() AS checked_at")
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| failure("database_clock", &error))?;
        let checked_at: DateTime<Utc> =
            clock.try_get("checked_at").map_err(|error| failure("database_clock", &error))?;

        let rows = sqlx::query(
            "SELECT id AS delivery_id, sending_at, lease_expires_at
             FROM public.deliveries
             WHERE status = 'sending'
               AND lease_token IS NOT NULL
               AND lease_expires_at > $1
             ORDER BY sending_at, id",
        )
        .bind(checked_at)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| failure("mail_sends", &error))?;
        let mut mail_sends = Vec::with_capacity(rows.len());
        for row in rows {
            mail_sends.push(InFlightMailSend {
                delivery_id: row
                    .try_get("delivery_id")
                    .map_err(|error| failure("mail_sends", &error))?,
                sending_at: row
                    .try_get("sending_at")
                    .map_err(|error| failure("mail_sends", &error))?,
                lease_expires_at: row
                    .try_get("lease_expires_at")
                    .map_err(|error| failure("mail_sends", &error))?,
            });
        }

        let rows = sqlx::query(
            "SELECT notification_id, sending_at, lease_expires_at
             FROM public.lot_watch_pm_deliveries
             WHERE status = 'sending'
               AND lease_token IS NOT NULL
               AND lease_expires_at > $1
             ORDER BY sending_at, notification_id",
        )
        .bind(checked_at)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| failure("pm_sends", &error))?;
        let mut pm_sends = Vec::with_capacity(rows.len());
        for row in rows {
            pm_sends.push(InFlightPmSend {
                notification_id: row
                    .try_get("notification_id")
                    .map_err(|error| failure("pm_sends", &error))?,
                sending_at: row
                    .try_get("sending_at")
                    .map_err(|error| failure("pm_sends", &error))?,
                lease_expires_at: row
                    .try_get("lease_expires_at")
                    .map_err(|error| failure("pm_sends", &error))?,
            });
        }

        transaction
            .commit()
            .await
            .map_err(|error| failure("finish_read_only_transaction", &error))?;
        Ok(PostgresInFlightSends { checked_at, mail_sends, pm_sends })
    }

    /// 读取高于 `after_version` 的迁移历史与兼容登记；登记缺失保持 NULL，由调用方失败关闭。
    pub async fn collect_migration_suffix(
        &self,
        after_version: i64,
    ) -> Result<PostgresMigrationSuffix, DeployEvidenceError> {
        if after_version < 0 {
            return Err(DeployEvidenceError {
                stage: "validate_after_version",
                sqlx_kind: "invalid_argument",
                sqlstate: "none".to_owned(),
            });
        }
        let mut transaction = self
            .pool()
            .begin_read_snapshot()
            .await
            .map_err(|error| failure("begin_read_only_transaction", &error))?;
        set_read_only_limits(&mut transaction)
            .await
            .map_err(|(stage, error)| failure(stage, &error))?;

        let row = sqlx::query(
            "SELECT COALESCE(max(version), 0) AS current_version
             FROM public._sqlx_migrations WHERE success",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| failure("current_version", &error))?;
        let current_version: i64 =
            row.try_get("current_version").map_err(|error| failure("current_version", &error))?;

        let rows = sqlx::query(
            "SELECT migration.version, migration.success, compatibility.level
             FROM public._sqlx_migrations AS migration
             LEFT JOIN public.schema_migration_compatibility AS compatibility
               ON compatibility.version = migration.version
             WHERE migration.version > $1
             ORDER BY migration.version",
        )
        .bind(after_version)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|error| failure("migration_suffix", &error))?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            entries.push(MigrationCompatibilityEntry {
                version: row
                    .try_get("version")
                    .map_err(|error| failure("migration_suffix", &error))?,
                success: row
                    .try_get("success")
                    .map_err(|error| failure("migration_suffix", &error))?,
                level: row.try_get("level").map_err(|error| failure("migration_suffix", &error))?,
            });
        }

        transaction
            .commit()
            .await
            .map_err(|error| failure("finish_read_only_transaction", &error))?;
        Ok(PostgresMigrationSuffix { current_version, entries })
    }
}

// 只携带变体名与校验后的 SQLSTATE，不保存或格式化原始错误。
fn failure(stage: &'static str, error: &sqlx::Error) -> DeployEvidenceError {
    DeployEvidenceError {
        stage,
        sqlx_kind: crate::database::sql_error_kind(error),
        sqlstate: crate::database::sql_error_sqlstate(error).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::MigrationCompatibilityEntry;

    /// 兼容后缀必须从 `from` 起连续、全部成功且逐条登记为 compatible，最后一条等于数据库版本。
    fn complete(entries: &[MigrationCompatibilityEntry], from: i64, database_version: i64) -> bool {
        if entries.is_empty() || database_version <= from {
            return false;
        }
        let mut previous = from;
        for entry in entries {
            if previous.checked_add(1) != Some(entry.version)
                || !entry.success
                || entry.level.as_deref() != Some("compatible")
            {
                return false;
            }
            previous = entry.version;
        }
        previous == database_version
    }

    fn entry(version: i64, level: Option<&str>) -> MigrationCompatibilityEntry {
        MigrationCompatibilityEntry { version, success: true, level: level.map(str::to_owned) }
    }

    #[test]
    fn compatible_suffix_requires_continuous_registered_versions() {
        assert!(complete(&[entry(96, Some("compatible")), entry(97, Some("compatible"))], 95, 97));
        assert!(!complete(&[entry(96, Some("compatible")), entry(98, Some("compatible"))], 95, 98));
        assert!(!complete(&[entry(96, Some("compatible"))], 95, 97));
        assert!(!complete(&[entry(96, Some("breaking"))], 95, 96));
        assert!(!complete(&[entry(96, None)], 95, 96));
        assert!(!complete(&[], 95, 95));
        let mut failed = entry(96, Some("compatible"));
        failed.success = false;
        assert!(!complete(&[failed], 95, 96));
    }
}
