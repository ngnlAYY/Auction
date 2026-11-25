//! 寄售运维：存量合同的权利状态补刷。
//!
//! 合同在订单阶段写口接入同事务权利刷新之前建立时，权利状态可能停在登记时的版本。这里经 Admin
//! 订单读口逐页列出全部合同（成交与撤回），对每个合同调用存储的 `refresh_contract_rights`；
//! SQL、锁与状态计算都留在存储与领域，本文件只编排遍历与输出。
//!
//! 不变量 8：只追加新的状态版本，不改写已有版本；刷新与续办共用权利锁，重复执行不追加（幂等）。
//! 订单页按 `created_at DESC, contract_id DESC` 排序，创建时间不可变：遍历期间新建的合同只会排到前面，
//! 把已读过的行推后，造成重复而不会跳过；重复刷新返回 0。是否覆盖完整按不同 `contract_id` 计数。

use std::collections::HashSet;

use anyhow::Context;
use auction_application::consignment::{AdminOrderQuery, AdminOrderRead, ListingPageWindow};
use auction_storage::{DatabaseConnectionPurpose, PgRightsStore};
use serde_json::json;

use crate::{app::print_json, cli::ConsignmentCommand, config_dir::ConfigDir};

/// 订单读口允许的最大页长。
const PAGE_SIZE: u32 = 200;

pub(crate) async fn run(config_dir: &ConfigDir, command: ConsignmentCommand) -> anyhow::Result<()> {
    match command {
        ConsignmentCommand::RefreshRights => {
            super::with_worker_store(
                config_dir,
                DatabaseConnectionPurpose::CliOperations,
                async |_, store| {
                    // 补刷不发码、不核验，不需要挑战码密码端口。
                    let rights = PgRightsStore::new(store.as_ref().clone(), None);
                    let mut page = 1;
                    let mut total = 0;
                    let mut refreshed = HashSet::new();
                    let mut appended: u64 = 0;
                    // 超出读口分页上限时窗口构造失败即停止；下方覆盖核对按未完成退出，不静默跳过。
                    while let Ok(window) = ListingPageWindow::new(Some(page), Some(PAGE_SIZE)) {
                        let orders = store
                            .list_admin_orders(AdminOrderQuery { window, world: None, state: None })
                            .await
                            .context("读取寄售订单页失败")?;
                        total = orders.total;
                        if orders.items.is_empty() {
                            break;
                        }
                        for order in orders.items {
                            let contract_id = order.core.contract_id;
                            appended += rights
                                .refresh_contract_rights(contract_id)
                                .await
                                .with_context(|| format!("补刷合同 {contract_id} 的权利失败"))?;
                            refreshed.insert(contract_id);
                        }
                        page += 1;
                    }
                    let distinct =
                        u64::try_from(refreshed.len()).context("已补刷合同数超出可表示范围")?;
                    let complete = distinct >= total;
                    print_json(&json!({
                        "orders_total": total,
                        "contracts_refreshed": distinct,
                        "states_appended": appended,
                        "complete": complete,
                    }))?;
                    anyhow::ensure!(complete, "订单数超出分页上限，仍有合同未补刷");
                    Ok(())
                },
            )
            .await
        }
    }
}
