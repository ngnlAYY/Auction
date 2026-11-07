//! 只读快照：Item Shop Credits 余额与 Armory 装备身份集合。
//!
//! 与收件读取共用只读凭据会话与抓取预算；不执行 organize/解锁/寄送等任何写操作。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use chrono::{DateTime, Utc};

use crate::diagnostics as safe_diagnostics;

use super::error::fetch_budget_exhausted;
use super::*;

impl MoogleMailGateway {
    /// 读取并校验一个 world 的 Item Shop Credits 余额。
    pub async fn read_balance(
        &self,
        world: MoogleMailWorld,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailBalanceSnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Balance,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session =
                self.credentials.begin_read(world, MoogleMailOperation::Balance).await?;
            let result = async {
                let target = self.client.balance(world)?;
                let document = self.client.get(&target, session.cookie()).await?;
                self.client
                    .diagnose_parse(
                        &target,
                        &document,
                        self.parser.parse_balance(&document, world, observed_at),
                    )
                    .await
            }
            .await;
            match result {
                Ok(snapshot) => {
                    session.check_current().await?;
                    Ok(snapshot)
                }
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_balance_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }

    /// 只读读取一个 world、一个已批准分类的完整 Armory 装备身份集合。
    ///
    /// 与 `read_balance` 共用只读凭据会话与抓取预算；不执行 organize/解锁/寄送等任何写操作。
    pub async fn read_inventory_ids(
        &self,
        world: MoogleMailWorld,
        category: MoogleMailInventoryCategory,
        observed_at: DateTime<Utc>,
    ) -> Result<MoogleMailInventorySnapshot, MoogleMailError> {
        if observed_at.timestamp_nanos_opt().is_none() {
            return Err(operation_error(
                MoogleMailOperation::Inventory,
                world,
                MoogleMailErrorCategory::Configuration,
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.config.fetch_budget)
            .ok_or_else(|| configuration(world))?;
        let result = tokio::time::timeout_at(deadline, async {
            let mut session =
                self.credentials.begin_read(world, MoogleMailOperation::Inventory).await?;
            let result = async {
                let target = self.client.inventory(world, category);
                let document = self.client.get(&target, session.cookie()).await?;
                self.client
                    .diagnose_parse(
                        &target,
                        &document,
                        self.parser.parse_inventory_ids(&document, world, category, observed_at),
                    )
                    .await
            }
            .await;
            match result {
                Ok(snapshot) => {
                    session.check_current().await?;
                    Ok(snapshot)
                }
                Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
                    session.invalidate_current().await?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        })
        .await
        .map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_read",
                "read_inventory_timeout",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            fetch_budget_exhausted(world)
        })?;
        if tokio::time::Instant::now() >= deadline {
            return Err(fetch_budget_exhausted(world));
        }
        result
    }
}
