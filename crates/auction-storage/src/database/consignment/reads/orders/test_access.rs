//! 订单阶段变更入口的 PG 测试窄入口；只在 `test-db` 构建。

use auction_application::consignment::ConsignmentError;
use sqlx::PgConnection;
use uuid::Uuid;

use super::events::record_order_stage_change_in_tx;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;

impl DatabaseStore {
    /// 在测试持有的事务里调用订单阶段变更入口（事件 + 权利刷新）；提交或回滚由测试决定。
    pub async fn test_record_order_stage_change(
        conn: &mut PgConnection,
        contract_id: Uuid,
    ) -> Result<(), ConsignmentError> {
        let now = database_now(&mut *conn).await.map_err(WriteError::into_application)?;
        record_order_stage_change_in_tx(conn, contract_id, now)
            .await
            .map_err(WriteError::into_application)
    }
}
