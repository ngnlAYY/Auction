//! PostgreSQL 公开履约进度查询：同一只读快照内读取场次与读取时刻、总数、本页 lot 事实与其全部
//! 结算义务。
//!
//! 只选判定与展示需要的列：投递、履约与命令标识、邮件标识、失败码、证明与资金字段都不读取；
//! recovery 的阻断码只作判定输入，人工对账投递与重挂目标只取“是否存在”。不连接代理授权表。

use std::collections::HashMap;

use auction_application::core::ApplicationError;
use auction_application::public::{
    PublicFulfillmentAuction, PublicFulfillmentLot, PublicFulfillmentObligation,
    PublicFulfillmentPage, PublicFulfillmentRecovery, PublicFulfillmentRequest,
};
use auction_time::Timestamp;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::public::lot_code::public_lot_code_sql;
use crate::database::storage_error;
use crate::database::storage_error_from;

/// 本页与 total 共用的 lot 来源；筛选里的 lot 代码需要装备分类。
macro_rules! fulfillment_lot_source {
    () => {
        "FROM public.lots AS l
         JOIN public.equipment AS e ON e.id = l.equipment_id
"
    };
}

/// 本页与 total 共用的可见集合：只含公开过的成交与流拍 lot，撤拍 lot 不进入任何公开读取。按
/// $1 拍卖、$2 lot、$3 lot 代码绑定；total 必须复用同一条件，避免页数与本页错位。
macro_rules! fulfillment_lot_filters {
    () => {
        concat!(
            "WHERE l.auction_id = $1
           AND l.status IN ('sold', 'unsold')
           AND ($2::uuid IS NULL OR l.id = $2)
           AND ($3::text IS NULL OR ",
            public_lot_code_sql!("e", "l"),
            " LIKE '%' || lower($3) || '%')
"
        )
    };
}

/// 在同一只读快照中读取已结算场次的履约事实；场次不存在、未结算、不可见或世界不符时返回
/// `None`。可见性与竞价日志一致：停用世界的已结算场次仍公开。
pub(super) async fn list(
    store: &DatabaseStore,
    request: PublicFulfillmentRequest,
) -> Result<Option<PublicFulfillmentPage>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启公开履约进度读取事务失败"))?;

    // 读取时刻取快照事务的 now()：逾期判定与本页事实出自同一时刻。
    let auction = sqlx::query(
        "SELECT a.id, a.world_id, a.sequence, now() AS read_at
         FROM public.auctions AS a
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         WHERE a.id = $1
           AND a.status = 'settled'
           AND ($2::text IS NULL OR a.world_id = $2)",
    )
    .bind(request.auction_id)
    .bind(request.world_id.as_deref())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取公开履约进度场次失败"))?;
    let Some(auction) = auction else {
        transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交公开履约进度空结果事务失败"))?;
        return Ok(None);
    };
    let read_at: Timestamp = auction
        .try_get("read_at")
        .map_err(|error| storage_error_from(error, "读取公开履约进度读取时刻失败"))?;
    let auction = PublicFulfillmentAuction {
        auction_id: auction
            .try_get("id")
            .map_err(|error| storage_error_from(error, "读取公开履约进度场次标识失败"))?,
        world_id: auction
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取公开履约进度场次世界失败"))?,
        sequence: auction
            .try_get("sequence")
            .map_err(|error| storage_error_from(error, "读取公开履约进度场次序号失败"))?,
    };

    let total = sqlx::query_scalar::<_, i64>(concat!(
        "SELECT count(*)::bigint ",
        fulfillment_lot_source!(),
        fulfillment_lot_filters!()
    ))
    .bind(request.auction_id)
    .bind(request.lot_id)
    .bind(request.lot_code.as_deref())
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "统计公开履约进度 lot 失败"))?;

    let rows = sqlx::query(concat!(
        "SELECT
            l.id AS lot_id,
            l.category_number AS lot_number,
            l.status AS lot_status,
            e.name AS equipment_name,
            e.category AS equipment_category,
            COALESCE(submission.seller, '—')::text AS seller_name,
            buyer.username AS buyer_name,
            settlement.end_price AS final_price,
            loss.lot_id IS NOT NULL AS equipment_lost,
            recovery.stage AS recovery_stage,
            recovery.resolution AS recovery_resolution,
            recovery.payment_deadline_at AS recovery_payment_deadline_at,
            recovery.cod_return_count AS recovery_cod_return_count,
            recovery.cod_return_limit AS recovery_cod_return_limit,
            recovery.blocking_code AS recovery_blocking_code,
            recovery.manual_reconcile_delivery_id IS NOT NULL AS recovery_manual_reconcile,
            recovery.relisted_lot_id IS NOT NULL AS recovery_relisted
         ",
        fulfillment_lot_source!(),
        "         LEFT JOIN public.users AS buyer ON buyer.id = l.leader_user_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
         LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = l.id
         LEFT JOIN public.settlement_equipment_losses AS loss ON loss.lot_id = l.id
         LEFT JOIN public.settlement_recoveries AS recovery
           ON recovery.auction_id = l.auction_id AND recovery.lot_id = l.id
         ",
        fulfillment_lot_filters!(),
        "         ORDER BY l.lot_number, l.id
         LIMIT $4 OFFSET $5"
    ))
    .bind(request.auction_id)
    .bind(request.lot_id)
    .bind(request.lot_code.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取公开履约进度 lot 失败"))?;

    let lot_ids = rows
        .iter()
        .map(|row| {
            row.try_get::<Uuid, _>("lot_id")
                .map_err(|error| storage_error_from(error, "读取公开履约进度 lot 标识失败"))
        })
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    // 义务按 (auction_id, lot_id, obligation_kind) 唯一键读取，只覆盖本页 lot。
    let obligation_rows = sqlx::query(
        "SELECT
            o.lot_id,
            o.obligation_kind,
            o.business_state,
            f.execution_state,
            d.status AS delivery_status
         FROM public.settlement_obligations AS o
         LEFT JOIN public.settlement_obligation_fulfillments AS f ON f.obligation_id = o.id
         LEFT JOIN public.deliveries AS d ON d.id = f.delivery_id
         WHERE o.auction_id = $1 AND o.lot_id = ANY($2)
         ORDER BY o.lot_id, o.obligation_kind",
    )
    .bind(request.auction_id)
    .bind(&lot_ids)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取公开履约进度义务失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交公开履约进度读取事务失败"))?;

    let mut obligations = HashMap::<Uuid, Vec<PublicFulfillmentObligation>>::new();
    for row in &obligation_rows {
        let (lot_id, obligation) = map_obligation(row)?;
        obligations.entry(lot_id).or_default().push(obligation);
    }
    let entries = rows
        .iter()
        .zip(lot_ids)
        .map(|(row, lot_id)| map_lot(row, lot_id, obligations.remove(&lot_id).unwrap_or_default()))
        .collect::<Result<Vec<_>, ApplicationError>>()?;
    let page = PublicFulfillmentPage {
        auction,
        read_at,
        entries,
        total,
        limit: request.limit,
        offset: request.offset,
    };
    page.validate().map_err(storage_error)?;
    Ok(Some(page))
}

fn map_obligation(row: &PgRow) -> Result<(Uuid, PublicFulfillmentObligation), ApplicationError> {
    let lot_id = row
        .try_get("lot_id")
        .map_err(|error| storage_error_from(error, "读取公开履约义务 lot 标识失败"))?;
    Ok((
        lot_id,
        PublicFulfillmentObligation {
            kind: row
                .try_get("obligation_kind")
                .map_err(|error| storage_error_from(error, "读取公开履约义务种类失败"))?,
            business_state: row
                .try_get("business_state")
                .map_err(|error| storage_error_from(error, "读取公开履约义务业务状态失败"))?,
            execution_state: row
                .try_get("execution_state")
                .map_err(|error| storage_error_from(error, "读取公开履约执行状态失败"))?,
            delivery_status: row
                .try_get("delivery_status")
                .map_err(|error| storage_error_from(error, "读取公开履约投递状态失败"))?,
        },
    ))
}

fn map_lot(
    row: &PgRow,
    lot_id: Uuid,
    obligations: Vec<PublicFulfillmentObligation>,
) -> Result<PublicFulfillmentLot, ApplicationError> {
    Ok(PublicFulfillmentLot {
        lot_id,
        lot_number: row
            .try_get("lot_number")
            .map_err(|error| storage_error_from(error, "读取公开履约 lot 序号失败"))?,
        equipment_name: row
            .try_get("equipment_name")
            .map_err(|error| storage_error_from(error, "读取公开履约装备名称失败"))?,
        equipment_category: row
            .try_get("equipment_category")
            .map_err(|error| storage_error_from(error, "读取公开履约装备分类失败"))?,
        seller_name: row
            .try_get("seller_name")
            .map_err(|error| storage_error_from(error, "读取公开履约卖家失败"))?,
        lot_status: row
            .try_get("lot_status")
            .map_err(|error| storage_error_from(error, "读取公开履约 lot 状态失败"))?,
        buyer_name: row
            .try_get("buyer_name")
            .map_err(|error| storage_error_from(error, "读取公开履约买家失败"))?,
        final_price: row
            .try_get("final_price")
            .map_err(|error| storage_error_from(error, "读取公开履约成交价失败"))?,
        equipment_lost: row
            .try_get("equipment_lost")
            .map_err(|error| storage_error_from(error, "读取公开履约装备损失标记失败"))?,
        recovery: map_recovery(row)?,
        obligations,
    })
}

/// recovery 由 LEFT JOIN 取得：阶段为空即没有 recovery 行；有行时其余必填列都必须存在。
fn map_recovery(row: &PgRow) -> Result<Option<PublicFulfillmentRecovery>, ApplicationError> {
    let stage: Option<String> = row
        .try_get("recovery_stage")
        .map_err(|error| storage_error_from(error, "读取公开履约 recovery 阶段失败"))?;
    let Some(stage) = stage else {
        return Ok(None);
    };
    let payment_deadline_at: Option<Timestamp> = row
        .try_get("recovery_payment_deadline_at")
        .map_err(|error| storage_error_from(error, "读取公开履约付款期限失败"))?;
    let cod_return_count: Option<i32> = row
        .try_get("recovery_cod_return_count")
        .map_err(|error| storage_error_from(error, "读取公开履约 COD 退回次数失败"))?;
    let cod_return_limit: Option<i32> = row
        .try_get("recovery_cod_return_limit")
        .map_err(|error| storage_error_from(error, "读取公开履约 COD 退回上限失败"))?;
    let (Some(payment_deadline_at), Some(cod_return_count), Some(cod_return_limit)) =
        (payment_deadline_at, cod_return_count, cod_return_limit)
    else {
        return Err(storage_error("公开履约 recovery 必填列缺失"));
    };
    Ok(Some(PublicFulfillmentRecovery {
        stage,
        resolution: row
            .try_get("recovery_resolution")
            .map_err(|error| storage_error_from(error, "读取公开履约撤销结论失败"))?,
        payment_deadline_at,
        cod_return_count,
        cod_return_limit,
        blocking_code: row
            .try_get("recovery_blocking_code")
            .map_err(|error| storage_error_from(error, "读取公开履约 recovery 阻断失败"))?,
        manual_reconcile: row
            .try_get("recovery_manual_reconcile")
            .map_err(|error| storage_error_from(error, "读取公开履约人工对账标记失败"))?,
        relisted: row
            .try_get("recovery_relisted")
            .map_err(|error| storage_error_from(error, "读取公开履约重挂标记失败"))?,
    }))
}
