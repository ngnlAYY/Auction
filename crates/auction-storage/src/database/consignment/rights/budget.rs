//! 找回与发码的频率预算（J11）：每浏览器绑定与每可信客户端 IP 的滚动 10 分钟窗口。
//!
//! 不变量 5、6：窗口计数来自持久的“命令命中”行（命令 ID 为主键，同命令只计一次，重放不重复计入），
//! 内存只用于唤醒。浏览器与 IP 的锁在调用前已按固定顺序取得，因此“读计数再写命中”在并发下精确。
//! 超限只返回 `BudgetExceeded` 并回滚，不写任何行；是否超限与目标真实与否无关。
//! 命中行只存浏览器绑定摘要与 IP 的域分隔 SHA-256（IPv6 取 /64 前缀），不存明文 IP。

use std::net::IpAddr;

use auction_application::consignment::{BrowserBinding, RightsStoreError};
use auction_domain::consignment::rights::{BUDGET_WINDOW, request_within_budget};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use super::TxError;

/// 可信客户端 IP 的预算键：IPv4 与映射地址取 4 字节，IPv6 取 /64 前缀，再做域分隔摘要。
pub(super) fn ip_digest(ip: IpAddr) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"auction.consignment.right.budget-ip.v1\0");
    match ip {
        IpAddr::V4(address) => hash.update(address.octets()),
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(mapped) => hash.update(mapped.octets()),
            None => hash.update(&address.octets()[..8]),
        },
    }
    hash.finalize().into()
}

/// 先核对两道预算，未超限才追加本命令的命中行。
pub(super) async fn spend(
    connection: &mut PgConnection,
    command_id: Uuid,
    browser: BrowserBinding,
    ip: IpAddr,
    now: Timestamp,
) -> Result<(), TxError> {
    let since = now.checked_sub_signed(BUDGET_WINDOW).ok_or(RightsStoreError::Unavailable)?;
    let ip = ip_digest(ip);
    let (browser_used, ip_used): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE browser_binding_digest = $1),
                count(*) FILTER (WHERE client_ip_digest = $2)
         FROM public.consignment_right_request_hits
         WHERE occurred_at > $3 AND (browser_binding_digest = $1 OR client_ip_digest = $2)",
    )
    .bind(browser.0.as_slice())
    .bind(ip.as_slice())
    .bind(since)
    .fetch_one(&mut *connection)
    .await?;
    request_within_budget(saturate(browser_used), saturate(ip_used))
        .map_err(RightsStoreError::from)?;
    sqlx::query(
        "INSERT INTO public.consignment_right_request_hits
            (command_id, browser_binding_digest, client_ip_digest, occurred_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(command_id)
    .bind(browser.0.as_slice())
    .bind(ip.as_slice())
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

fn saturate(count: i64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}
