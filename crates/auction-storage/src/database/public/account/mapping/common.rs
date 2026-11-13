//! 账户读取共用的分页与请求校验。

use crate::database::storage_error_from;

pub(in crate::database::public::account) use crate::database::storage_error;
use auction_application::auction::{PublicMyBidsPage, PublicMyBidsRequest};
use auction_application::core::ApplicationError;
use sqlx::Row;
use uuid::Uuid;

pub(in crate::database::public::account) fn validate_page(
    page: PublicMyBidsPage,
    request: PublicMyBidsRequest,
) -> Result<PublicMyBidsPage, ApplicationError> {
    let bid_count =
        i64::try_from(page.bids.len()).map_err(|_| storage_error("账户竞价页大小溢出"))?;
    let proxy_count =
        i64::try_from(page.proxies.len()).map_err(|_| storage_error("账户代理页大小溢出"))?;
    if page.bid_total < 0
        || page.proxy_total < 0
        || bid_count > request.bids.limit
        || proxy_count > request.proxies.limit
        || bid_count > page.bid_total
        || proxy_count > page.proxy_total
        || page.proxies.iter().any(|proxy| proxy.user_id.is_nil())
    {
        return Err(storage_error("账户竞价页结果不满足边界约束"));
    }
    Ok(page)
}

pub(in crate::database::public::account) fn validate_account_request(
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<(), ApplicationError> {
    if user_id.is_nil() {
        return Err(storage_error("公开账户用户标识无效"));
    }
    request.validate().map_err(storage_error)
}

pub(in crate::database::public::account) fn get_pg<T>(
    row: &sqlx::postgres::PgRow,
    column: &str,
    label: &'static str,
) -> Result<T, ApplicationError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(|error| storage_error_from(error, label))
}
