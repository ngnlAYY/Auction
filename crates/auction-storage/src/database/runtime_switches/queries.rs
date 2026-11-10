//! 先取最新事实再校验；未来名称在值解码之前忽略。

use super::super::DatabaseStore;
use auction_application::runtime_switches::{
    RuntimeSwitchError, RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchRead,
};
use sqlx::{Row, postgres::PgRow};

pub(super) const LATEST: &str = "SELECT * FROM public.runtime_switch_events WHERE switch_name=$1 ORDER BY revision DESC LIMIT 1";

pub(super) async fn read(store: &DatabaseStore) -> Result<RuntimeSwitchRead, RuntimeSwitchError> {
    let names: Vec<_> = RuntimeSwitchName::ALL.iter().map(|name| name.label()).collect();
    // 左连接保留空表的数据库时间；每名称一次索引查找，不从过期行回溯旧覆盖。
    let rows = sqlx::query(
        "SELECT transaction_timestamp() AS database_time, latest.*
        FROM unnest($1::text[]) AS known(switch_name)
        LEFT JOIN LATERAL (SELECT * FROM public.runtime_switch_events
            WHERE switch_name=known.switch_name ORDER BY revision DESC LIMIT 1) AS latest ON true",
    )
    .bind(names)
    .fetch_all(store.pool())
    .await
    .map_err(unavailable)?;
    let first = rows.first().ok_or(RuntimeSwitchError::Corrupt)?;
    let database_time = first.try_get("database_time").map_err(corrupt)?;
    let mut events = Vec::new();
    for row in rows {
        let name: Option<String> = row.try_get("switch_name").map_err(corrupt)?;
        if name.as_deref().and_then(RuntimeSwitchName::parse).is_none() {
            continue;
        }
        events.push(map(&row)?);
    }
    Ok(RuntimeSwitchRead { database_time, events })
}

pub(super) fn map(row: &PgRow) -> Result<RuntimeSwitchEvent, RuntimeSwitchError> {
    let decode = || -> Result<RuntimeSwitchEvent, sqlx::Error> {
        let name: String = row.try_get("switch_name")?;
        let operation: String = row.try_get("operation")?;
        Ok(RuntimeSwitchEvent {
            command_id: row.try_get("command_id")?,
            name: RuntimeSwitchName::parse(&name).ok_or(sqlx::Error::RowNotFound)?,
            revision: row.try_get("revision")?,
            operation: serde_json::from_value(serde_json::Value::String(operation))
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            value: row.try_get("value")?,
            expires_at: row.try_get("expires_at")?,
            expired_command_id: row.try_get("expired_command_id")?,
            actor_id: row.try_get("actor_id")?,
            entrypoint: row.try_get("entrypoint")?,
            reason: row.try_get("reason")?,
            recorded_at: row.try_get("recorded_at")?,
        })
    };
    let event = decode().map_err(corrupt)?;
    event.validate()?;
    let result: serde_json::Value = row.try_get("result_payload").map_err(corrupt)?;
    if serde_json::to_value(&event).map_err(corrupt)? != result {
        return Err(RuntimeSwitchError::Corrupt);
    }
    Ok(event)
}

fn corrupt<T>(_: T) -> RuntimeSwitchError {
    RuntimeSwitchError::Corrupt
}

fn unavailable<T>(_: T) -> RuntimeSwitchError {
    RuntimeSwitchError::Unavailable
}
