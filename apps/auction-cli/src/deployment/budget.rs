//! 三角色实际池上限与运维声明的离线预算；不连接数据库，也不声称已核实服务器 reserve。

use anyhow::Context;
use auction_config::{AppConfig, DatabaseProfileMode, Role};
use serde::Serialize;

/// 同一配置目录一组三角色的连接预算表。
#[derive(Serialize)]
pub(super) struct DatabaseBudgetReport {
    roles: Vec<RoleBudget>,
    max_concurrent_cli: u32,
    maintenance_pool_max_connections: u32,
    maintenance_connections: u64,
    external_reserved_connections: u32,
    planned_connections: u64,
    max_connections: u32,
    superuser_reserved_connections: u32,
    reserved_connections: u32,
    ordinary_connection_limit: u64,
    remaining_connections: u64,
    reserve_values_verified: bool,
}

#[derive(Serialize)]
struct RoleBudget {
    role: &'static str,
    max_connections: u32,
    long_lived_reserved_connections: u32,
    ordinary_work_connections: u32,
}

pub(super) fn check(
    public: &AppConfig,
    admin: &AppConfig,
    worker: &AppConfig,
) -> anyhow::Result<DatabaseBudgetReport> {
    let profiles = worker.database_profiles;
    if public.database_profiles != profiles || admin.database_profiles != profiles {
        anyhow::bail!("三角色数据库分档快照不一致，请在公共配置稳定后重试");
    }
    let legacy = profiles.mode == DatabaseProfileMode::Legacy;
    let roles = [public, admin, worker]
        .into_iter()
        .map(|config| {
            let reserved = match config.role {
                Role::Public => 1,
                Role::Admin => 0,
                Role::Worker => 3,
            };
            let maximum = if legacy { 16 } else { config.database_pool.max_connections };
            RoleBudget {
                role: config.role.name(),
                max_connections: maximum,
                long_lived_reserved_connections: reserved,
                ordinary_work_connections: maximum.saturating_sub(reserved),
            }
        })
        .collect::<Vec<_>>();
    let budget = profiles.budget;
    let maintenance_max = if legacy { 16 } else { profiles.maintenance.max_connections };
    let maintenance = u64::from(maintenance_max)
        .checked_mul(u64::from(budget.max_concurrent_cli))
        .context("维护连接预算溢出")?;
    let planned = roles
        .iter()
        .map(|role| u64::from(role.max_connections))
        .sum::<u64>()
        .checked_add(maintenance)
        .and_then(|value| value.checked_add(u64::from(budget.external_reserved_connections)))
        .context("数据库连接总预算溢出")?;
    let limit = u64::from(budget.max_connections)
        .saturating_sub(u64::from(budget.superuser_reserved_connections))
        .saturating_sub(u64::from(budget.reserved_connections));
    if !legacy && planned > limit {
        anyhow::bail!(
            "数据库预算超限：计划 {planned} 个普通连接，上限 {limit}；请调整池、并行 CLI 或预留"
        );
    }
    Ok(DatabaseBudgetReport {
        roles,
        max_concurrent_cli: budget.max_concurrent_cli,
        maintenance_pool_max_connections: maintenance_max,
        maintenance_connections: maintenance,
        external_reserved_connections: budget.external_reserved_connections,
        planned_connections: planned,
        max_connections: budget.max_connections,
        superuser_reserved_connections: budget.superuser_reserved_connections,
        reserved_connections: budget.reserved_connections,
        ordinary_connection_limit: limit,
        remaining_connections: limit.saturating_sub(planned),
        reserve_values_verified: false,
    })
}
