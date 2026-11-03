//! Admin lot 列表、详情及生命周期只读投影；不拼装默认字段冒充权威状态。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::{
    AdminLotListRequest, AdminLotPage, AdminLotReader, ApplicationError, ApplicationFuture,
};
#[cfg(feature = "postgres")]
use auction_application::{AdminLotSummary, SettlementAuctionSnapshot};
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, EquipmentId, LotId, ProxyAuthorizationId, SubmissionId, UserId};

use super::DatabaseStore;
use crate::database::storage_error;

impl AdminLotReader for DatabaseStore {
    fn list_admin_lots(
        &self,
        request: AdminLotListRequest,
    ) -> ApplicationFuture<'_, Option<AdminLotPage>> {
        Box::pin(async move {
            if !request.validate() {
                return Err(storage_error("Admin lot 列表参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
    request: AdminLotListRequest,
) -> Result<Option<AdminLotPage>, ApplicationError> {
    use chrono::{DateTime, Utc};
    use serde_json::Value;
    use sqlx::Row;

    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启 Admin lot 读取事务失败"))?;

    let auction_row = sqlx::query(
        "SELECT
            id, world_id, sequence, status, preview_scheduled_at, scheduled_open_at,
            preview_opened_at, preview_manual_open, preview_revision, opened_at,
            final_phase_at, settled_at, cancelled_at, cancelled_reason, cancelled_by,
            state_version, scheduled_end_at, effective_end_at, created_at, updated_at
         FROM public.auctions
         WHERE id = $1",
    )
    .bind(request.auction_id.as_uuid())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 Admin lot 所属拍卖失败"))?;
    let Some(auction_row) = auction_row else {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交 Admin lot 空结果失败"))?;
        return Ok(None);
    };

    let auction = SettlementAuctionSnapshot {
        id: AuctionId::from_uuid(
            auction_row
                .try_get("id")
                .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖标识失败"))?,
        ),
        world_id: auction_row
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖世界失败"))?,
        sequence: auction_row
            .try_get("sequence")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖序号失败"))?,
        status: auction_row
            .try_get("status")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖状态失败"))?,
        preview_scheduled_at: auction_row
            .try_get::<DateTime<Utc>, _>("preview_scheduled_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 预览计划时间失败"))?,
        scheduled_open_at: auction_row
            .try_get::<DateTime<Utc>, _>("scheduled_open_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 开放计划时间失败"))?,
        preview_opened_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("preview_opened_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 预览开放时间失败"))?,
        preview_manual_open: auction_row
            .try_get("preview_manual_open")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 手动开放标记失败"))?,
        preview_revision: auction_row
            .try_get("preview_revision")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 预览版本失败"))?,
        opened_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("opened_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 实际开放时间失败"))?,
        final_phase_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("final_phase_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot最终阶段时间失败"))?,
        settled_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("settled_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 结算时间失败"))?,
        cancelled_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("cancelled_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 取消时间失败"))?,
        cancelled_reason: auction_row
            .try_get("cancelled_reason")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 取消原因失败"))?,
        cancelled_by: auction_row
            .try_get("cancelled_by")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 取消操作者失败"))?,
        state_version: auction_row
            .try_get("state_version")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖状态版本失败"))?,
        scheduled_end_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("scheduled_end_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 计划结束时间失败"))?,
        effective_end_at: auction_row
            .try_get::<Option<DateTime<Utc>>, _>("effective_end_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 有效结束时间失败"))?,
        created_at: auction_row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖创建时间失败"))?,
        updated_at: auction_row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .map_err(|error| storage_error_from(error, "读取 Admin lot 拍卖更新时间失败"))?,
    };

    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM public.lots WHERE auction_id = $1")
        .bind(request.auction_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "读取 Admin lot 总数失败"))?;

    let rows = sqlx::query(
        "SELECT
            l.id, l.auction_id, l.equipment_id, l.submission_id,
            l.category_number AS lot_number, l.status,
            l.start_price, COALESCE(l.current_price, l.start_price) AS current_price,
            l.leader_user_id, l.effective_end_at, l.slow_due_at, l.slow_generation,
            l.slow_authorization_id, l.state_version, l.created_at, l.updated_at,
            e.external_equipment_id, e.name AS equipment_name, e.category AS equipment_category,
            e.attributes AS equipment_attributes, e.collected_at AS equipment_collected_at,
            s.seller, s.source_kind AS submission_source_kind,
            s.source_reference AS submission_source_reference, s.mailbox_key,
            s.external_mail_id, s.received_at AS submission_received_at
         FROM public.lots AS l
         INNER JOIN public.equipment AS e ON e.id = l.equipment_id
         LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
         WHERE l.auction_id = $1
         ORDER BY l.lot_number ASC, l.id ASC
         LIMIT $2 OFFSET $3",
    )
    .bind(request.auction_id.as_uuid())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 Admin lot 页面失败"))?;

    let entries = rows
        .into_iter()
        .map(|row| {
            let attributes: sqlx::types::Json<Value> = row
                .try_get("equipment_attributes")
                .map_err(|error| storage_error_from(error, "读取 Admin lot 装备属性失败"))?;
            Ok(AdminLotSummary {
                id: LotId::from_uuid(
                    row.try_get("id")
                        .map_err(|error| storage_error_from(error, "读取 Admin lot 标识失败"))?,
                ),
                auction_id: AuctionId::from_uuid(
                    row.try_get("auction_id").map_err(|error| {
                        storage_error_from(error, "读取 Admin lot 拍卖归属失败")
                    })?,
                ),
                equipment_id: EquipmentId::from_uuid(
                    row.try_get("equipment_id").map_err(|error| {
                        storage_error_from(error, "读取 Admin lot 装备归属失败")
                    })?,
                ),
                submission_id: row
                    .try_get::<Option<uuid::Uuid>, _>("submission_id")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 投稿归属失败"))?
                    .map(SubmissionId::from_uuid),
                lot_number: row
                    .try_get("lot_number")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 序号失败"))?,
                status: row
                    .try_get("status")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 状态失败"))?,
                start_price: row
                    .try_get("start_price")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 起始价失败"))?,
                current_price: row
                    .try_get("current_price")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 当前价失败"))?,
                leader_user_id: row
                    .try_get::<Option<uuid::Uuid>, _>("leader_user_id")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 领先用户失败"))?
                    .map(UserId::from_uuid),
                effective_end_at: row.try_get("effective_end_at").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 有效结束时间失败")
                })?,
                slow_due_at: row.try_get("slow_due_at").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 慢速代理时间失败")
                })?,
                slow_generation: row.try_get("slow_generation").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 慢速代理代数失败")
                })?,
                slow_authorization_id: row
                    .try_get::<Option<uuid::Uuid>, _>("slow_authorization_id")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 慢速代理授权失败"))?
                    .map(ProxyAuthorizationId::from_uuid),
                state_version: row
                    .try_get("state_version")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 状态版本失败"))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 创建时间失败"))?,
                updated_at: row
                    .try_get("updated_at")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 更新时间失败"))?,
                external_equipment_id: row.try_get("external_equipment_id").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 外部装备标识失败")
                })?,
                equipment_name: row
                    .try_get("equipment_name")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 装备名称失败"))?,
                equipment_category: row
                    .try_get("equipment_category")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 装备分类失败"))?,
                equipment_attributes: attributes.0,
                equipment_collected_at: row.try_get("equipment_collected_at").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 装备采集时间失败")
                })?,
                seller: row
                    .try_get("seller")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 卖家失败"))?,
                submission_source_kind: row.try_get("submission_source_kind").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 投稿来源类型失败")
                })?,
                submission_source_reference: row.try_get("submission_source_reference").map_err(
                    |error| storage_error_from(error, "读取 Admin lot 投稿来源引用失败"),
                )?,
                mailbox_key: row
                    .try_get("mailbox_key")
                    .map_err(|error| storage_error_from(error, "读取 Admin lot 邮箱标识失败"))?,
                external_mail_id: row.try_get("external_mail_id").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 外部邮件标识失败")
                })?,
                submission_received_at: row.try_get("submission_received_at").map_err(|error| {
                    storage_error_from(error, "读取 Admin lot 投稿接收时间失败")
                })?,
            })
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;

    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 Admin lot 读取事务失败"))?;

    let page =
        AdminLotPage { auction, entries, total, limit: request.limit, offset: request.offset };
    if !page.validate(request) {
        return Err(storage_error("Admin lot 存储返回不变量失败"));
    }
    Ok(Some(page))
}
