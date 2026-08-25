use super::*;

pub(in crate::database::account) fn validate_page(
    page: &PublicMyBidsPage,
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
    Ok(page.clone())
}

pub(in crate::database::account) fn validate_account_request(
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<(), ApplicationError> {
    if user_id.is_nil() {
        return Err(storage_error("公开账户用户标识无效"));
    }
    request.validate().map_err(storage_error)
}

#[cfg(feature = "postgres")]
pub(in crate::database::account) fn get_pg<T>(
    row: &sqlx::postgres::PgRow,
    column: &str,
    label: &str,
) -> Result<T, ApplicationError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn get_sqlite<T>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<T, ApplicationError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn get_sqlite_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&get_sqlite::<Vec<u8>>(row, column, label)?).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn get_sqlite_optional_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<Option<Uuid>, ApplicationError> {
    get_sqlite::<Option<Vec<u8>>>(row, column, label)?
        .map(|bytes| Uuid::from_slice(&bytes).map_err(|_| storage_error(label)))
        .transpose()
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn parse_timestamp(
    value: String,
    label: &str,
) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

pub(in crate::database::account) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
