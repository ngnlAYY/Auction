//! 迁移兼容后缀的唯一判定：部署回退门禁与 readiness 共用同一后缀查询与连续性规则。
//!
//! 后缀指高于旧程序期望版本的全部迁移历史；缺登记读为 NULL，一律不当作兼容。

use serde::Serialize;
use sqlx::PgConnection;

/// 一条高于期望版本的迁移历史及其兼容登记；`level` 缺失表示该版本没有登记，绝不能当作兼容。
#[derive(Debug, Serialize)]
pub struct MigrationCompatibilityEntry {
    pub version: i64,
    pub success: bool,
    pub level: Option<String>,
}

/// 兼容后缀不成立的首个原因；版本号只用于运维报错定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibleSuffixViolation {
    /// 数据库版本低于旧程序期望版本，旧程序会缺少已执行的迁移。
    DatabaseBehind,
    /// 该版本断号、未成功执行或未登记为 `compatible`。
    Incompatible(i64),
    /// 后缀没有连续覆盖到数据库版本。
    Incomplete,
}

/// 判定期望 `from` 的旧程序能否运行在 `database_version` 上。`entries` 必须是高于 `from` 的
/// 全部迁移历史（含失败记录）：从 `from + 1` 起逐版连续、执行成功且登记为 `compatible`，末尾
/// 等于数据库版本。两版本相等时只有空后缀成立，同版本之上残留的失败迁移记录同样拒绝。
pub fn check_compatible_suffix(
    entries: &[MigrationCompatibilityEntry],
    from: i64,
    database_version: i64,
) -> Result<(), CompatibleSuffixViolation> {
    if database_version < from {
        return Err(CompatibleSuffixViolation::DatabaseBehind);
    }
    let mut previous = from;
    for entry in entries {
        if previous.checked_add(1) != Some(entry.version)
            || !entry.success
            || entry.level.as_deref() != Some("compatible")
        {
            return Err(CompatibleSuffixViolation::Incompatible(entry.version));
        }
        previous = entry.version;
    }
    if previous != database_version {
        return Err(CompatibleSuffixViolation::Incomplete);
    }
    Ok(())
}

/// 读取高于 `after` 的全部迁移历史及兼容登记，按版本升序。必须由实际迁移 LEFT JOIN 登记驱动：
/// 缺表报错、缺登记为 NULL，均不得变成空集放行。
pub(in crate::database) async fn read_compatibility_suffix(
    connection: &mut PgConnection,
    after: i64,
) -> Result<Vec<MigrationCompatibilityEntry>, sqlx::Error> {
    let rows: Vec<(i64, bool, Option<String>)> = sqlx::query_as(
        "SELECT migration.version, migration.success, compatibility.level
         FROM public._sqlx_migrations AS migration
         LEFT JOIN public.schema_migration_compatibility AS compatibility
           ON compatibility.version = migration.version
         WHERE migration.version > $1
         ORDER BY migration.version",
    )
    .bind(after)
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, success, level)| MigrationCompatibilityEntry { version, success, level })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{CompatibleSuffixViolation, MigrationCompatibilityEntry, check_compatible_suffix};

    const COMPATIBLE: Option<&str> = Some("compatible");

    fn entry(version: i64, level: Option<&str>) -> MigrationCompatibilityEntry {
        MigrationCompatibilityEntry { version, success: true, level: level.map(str::to_owned) }
    }

    fn failed(version: i64) -> MigrationCompatibilityEntry {
        MigrationCompatibilityEntry { success: false, ..entry(version, COMPATIBLE) }
    }

    #[test]
    fn compatible_suffix_requires_continuous_successful_registered_versions() {
        assert_eq!(
            check_compatible_suffix(&[entry(96, COMPATIBLE), entry(97, COMPATIBLE)], 95, 97),
            Ok(())
        );
        assert_eq!(
            check_compatible_suffix(&[entry(96, COMPATIBLE), entry(98, COMPATIBLE)], 95, 98),
            Err(CompatibleSuffixViolation::Incompatible(98)),
            "跳号必须拒绝"
        );
        assert_eq!(
            check_compatible_suffix(&[entry(96, Some("breaking"))], 95, 96),
            Err(CompatibleSuffixViolation::Incompatible(96))
        );
        assert_eq!(
            check_compatible_suffix(&[entry(96, None)], 95, 96),
            Err(CompatibleSuffixViolation::Incompatible(96)),
            "未登记不能当作兼容"
        );
        assert_eq!(
            check_compatible_suffix(&[failed(96)], 95, 96),
            Err(CompatibleSuffixViolation::Incompatible(96))
        );
    }

    #[test]
    fn compatible_suffix_must_reach_the_database_version() {
        assert_eq!(
            check_compatible_suffix(&[entry(96, COMPATIBLE)], 95, 97),
            Err(CompatibleSuffixViolation::Incomplete)
        );
        assert_eq!(
            check_compatible_suffix(&[], 95, 96),
            Err(CompatibleSuffixViolation::Incomplete)
        );
        assert_eq!(
            check_compatible_suffix(&[], 95, 94),
            Err(CompatibleSuffixViolation::DatabaseBehind)
        );
    }

    #[test]
    fn equal_versions_pass_only_with_an_empty_suffix() {
        assert_eq!(check_compatible_suffix(&[], 95, 95), Ok(()));
        assert_eq!(
            check_compatible_suffix(&[failed(96)], 95, 95),
            Err(CompatibleSuffixViolation::Incompatible(96)),
            "同版本之上的失败迁移记录同样拒绝"
        );
    }
}
