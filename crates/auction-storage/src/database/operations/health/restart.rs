//! Admin 配置读取只取在线 Worker 的冷配置摘要。

use auction_application::admin::{
    WORKER_COLD_CONFIG_HEARTBEAT_KEY, WorkerColdConfigDigest, WorkerColdConfigReader,
};
use auction_application::core::ApplicationFuture;
use serde_json::Value;
use sqlx::types::Json;

use crate::database::{DatabaseStore, storage_error_from};

impl WorkerColdConfigReader for DatabaseStore {
    fn live_worker_cold_configs(
        &self,
    ) -> ApplicationFuture<'_, Vec<Option<WorkerColdConfigDigest>>> {
        Box::pin(async move {
            let mut transaction = self
                .pool()
                .begin_read_snapshot()
                .await
                .map_err(|error| storage_error_from(error, "开启 Worker 冷配置读取事务失败"))?;
            // starting 只证明有实例正在装配；stopping 已退出比较范围。持久序列围栏保证每实例只读最新观察。
            let rows: Vec<Option<Json<Value>>> = sqlx::query_scalar(
                "SELECT CASE WHEN status IN ('ready', 'degraded') THEN details -> $1::text END
                 FROM public.service_heartbeats
                 WHERE service_name = 'auction-worker' AND process_role = 'worker'
                   AND status <> 'stopping' AND expires_at > statement_timestamp()
                 ORDER BY instance_id",
            )
            .bind(WORKER_COLD_CONFIG_HEARTBEAT_KEY)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|error| storage_error_from(error, "读取 Worker 冷配置摘要失败"))?;
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "结束 Worker 冷配置读取事务失败"))?;
            Ok(rows
                .into_iter()
                .map(|digest| digest.and_then(|Json(value)| serde_json::from_value(value).ok()))
                .collect())
        })
    }
}
