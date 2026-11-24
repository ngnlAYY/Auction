//! v1 私有读模型的可表达性过滤：旧解码器表达不了的挂单整行隐藏，其余不受影响。
//!
//! 保护的不变量：旧列表的行与总数同一过滤、同一快照；旧详情对被隐藏的挂单与不存在同样返回未找到。

use auction_application::consignment::{
    AdminListingQuery, AdminListingRead, ListingPageWindow, OwnedListingRead, PrivateListingQuery,
};
use auction_domain::UserId;
use uuid::Uuid;

use super::support::{ISSUED, LISTING, USER_1, listing_awaiting_deposit, seed_accepted, succeeds};
use crate::support::TestDatabase;

const ADMIN_CLOSE: &str = "41000000-0000-4000-8000-0000000000b1";
const SEASON_CLOSE: &str = "41000000-0000-5000-8000-0000000000b2";
const SEASON_ID: &str = "2026-isekai-1";

/// 把 `listing_awaiting_deposit` 的固定标识换成第 `n` 套，得到同卖家的另一条待寄挂单。
fn extra_listing(n: u8, eid: &str) -> (String, String) {
    let id = format!("20000000-0000-0000-0000-0000000000{n}");
    let sql = listing_awaiting_deposit()
        .replace(LISTING, &id)
        .replace(
            "40000000-0000-4000-8000-000000000001",
            &format!("40000000-0000-4000-8000-0000000000{n}"),
        )
        .replace(
            "41000000-0000-5000-8000-000000000001",
            &format!("41000000-0000-5000-8000-0000000000{n}"),
        )
        .replace(
            "30000000-0000-0000-0000-000000000001",
            &format!("30000000-0000-0000-0000-0000000000{n}"),
        )
        .replace("'42'", &format!("'{eid}'"));
    (id, sql)
}

fn admin_close_sql(id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_id, listing_id, result_status, result_code,
             result_body, created_at)
         VALUES ('{ADMIN_CLOSE}', 2, 'admin', 'fake-admin', 'admin_delist', '{id}',
                 decode(repeat('ec', 32), 'hex'), 'listing', '{id}', '{id}', 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"admin_delist\",
                    \"disposition\":\"closed\",\"command_id\":\"{ADMIN_CLOSE}\",
                    \"listing_id\":\"{id}\",\"listing_version\":3,\"state\":\"closed\"}}',
                 '{ISSUED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{id}', 3, '{ADMIN_CLOSE}', 'admin', 'closed', 'awaiting_deposit', 'closed',
                 'admin_delisted', 101, '{ISSUED}');
         UPDATE public.consignment_listings
         SET state='closed', reason_code='admin_delisted', state_version=3, updated_at='{ISSUED}'
         WHERE id='{id}';
         INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id, result,
             source, metadata)
         VALUES (gen_random_uuid(), '{ISSUED}', 'admin', 'fake-admin', 'consignment.admin_delist',
                 'consignment_listing', '{id}', 'success', 'admin',
                 jsonb_build_object('command_id', '{ADMIN_CLOSE}',
                                    'admin_delist_reason', 'fake moderation note'));"
    )
}

/// 与 `close_season` 写入形状一致的季末关闭；这里的挂单没有代管 position，
/// 单独检验原因过滤（有 position 的行另由 position 过滤隐藏）。
fn season_close_sql(id: &str) -> String {
    format!(
        "INSERT INTO public.consignment_commands
            (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
             request_digest, target_kind, target_season_id, target_calendar_version,
             listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{SEASON_CLOSE}', 2, 'worker', 'fake-worker', 'season_close', '{SEASON_ID}',
                 decode(repeat('ee', 32), 'hex'), 'season', '{SEASON_ID}',
                 decode(repeat('ef', 32), 'hex'), NULL, 200, 'ok',
                 '{{\"schema_version\":2,\"operation\":\"season_close\",
                    \"disposition\":\"season_closed\",\"command_id\":\"{SEASON_CLOSE}\",
                    \"season_id\":\"{SEASON_ID}\",
                    \"target\":{{\"kind\":\"listing\",\"id\":\"{id}\"}},
                    \"closed_listing_ids\":[\"{id}\"]}}', '{ISSUED}');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{id}', 3, '{SEASON_CLOSE}', 'worker', 'season_closed', 'awaiting_deposit',
                 'closed', 'season_closed', 101, '{ISSUED}');
         UPDATE public.consignment_listings
         SET state='closed', reason_code='season_closed', state_version=3, updated_at='{ISSUED}'
         WHERE id='{id}';"
    )
}

fn query() -> PrivateListingQuery {
    PrivateListingQuery {
        window: ListingPageWindow::new(Some(1), Some(20)).expect("有效分页"),
        world: None,
        mode: None,
        state: None,
    }
}

#[tokio::test]
async fn postgres_v1_owned_list_hides_inexpressible_rows() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    // 正例行：待寄与运营下架；反例行：季末关闭（无 position）与整封接受后的代管态。
    let (awaiting, awaiting_sql) = extra_listing(51, "43");
    let (delisted, delisted_sql) = extra_listing(52, "44");
    let (season_closed, season_closed_sql) = extra_listing(53, "45");
    seed_accepted(pool).await;
    succeeds(pool, &(awaiting_sql + &delisted_sql + &season_closed_sql)).await;
    succeeds(pool, &admin_close_sql(&delisted)).await;
    succeeds(pool, &season_close_sql(&season_closed)).await;

    let seller = UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家"));
    // 季末关闭行与代管态行被隐藏，其余两行保留，总数同一过滤。
    let page = runtime.list_owned(seller, query()).await.expect("旧列表");
    assert_eq!((page.total, page.items.len()), (2, 2));
    let listed: Vec<Uuid> = page.items.iter().map(|row| row.listing().listing_id).collect();
    assert!(listed.contains(&Uuid::parse_str(&awaiting).expect("待寄")));
    assert!(listed.contains(&Uuid::parse_str(&delisted).expect("下架")));
    let hidden = Uuid::parse_str(&season_closed).expect("季末");
    assert!(!listed.contains(&hidden));
    assert!(runtime.owned_detail(seller, hidden).await.expect("详情").is_none());
    assert!(runtime.admin_detail(hidden).await.expect("Admin 详情").is_none());

    let custody = Uuid::parse_str(LISTING).expect("代管态挂单");
    assert!(runtime.owned_detail(seller, custody).await.expect("详情").is_none());
    assert!(runtime.admin_detail(custody).await.expect("Admin 详情").is_none());
    assert!(
        runtime
            .admin_detail(Uuid::parse_str(&awaiting).expect("待寄"))
            .await
            .expect("Admin 详情")
            .is_some()
    );

    let admin = runtime
        .list_admin(AdminListingQuery { listings: query(), seller_user_id: None })
        .await
        .expect("Admin 旧列表");
    assert_eq!(admin.total, 2);
    assert_eq!(admin.items.len(), 2);
}

/// 有过代管 position 的挂单即使以旧原因关闭也隐藏：实物可能仍在平台代管，
/// 旧页面显示成普通“已关闭”会误导卖家。存储态 `closed` 与原因 `admin_delisted` 都在旧集合内，
/// 只有“从未有过 position”谓词能挡住它。
///
/// 第 4 期的挂单触发器只允许代管后以 `season_closed` 关闭（原因过滤已隐藏），这个形状无法经生产路径
/// 产生；夹具沿用 `retirement.rs` 的做法，由维护账号临时停用挂单触发器改写存储态，
/// 生产触发器与读可见性都不变，运行账号读到的就是这一行。
#[tokio::test]
async fn postgres_v1_reads_hide_closed_listing_with_custody_position() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    seed_accepted(owner.postgres_pool()).await;
    succeeds(
        owner.postgres_pool(),
        &format!(
            "ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
             UPDATE public.consignment_listings
             SET state='closed', reason_code='admin_delisted', state_version=state_version+1,
                 updated_at='{ISSUED}'
             WHERE id='{LISTING}';
             ALTER TABLE public.consignment_listings ENABLE TRIGGER USER;"
        ),
    )
    .await;
    let positions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_custody_positions")
            .fetch_one(owner.postgres_pool())
            .await
            .expect("position 数");
    assert_eq!(positions, 1);

    let runtime = database.store().await.expect("以运行账号连接");
    let seller = UserId::from_uuid(Uuid::parse_str(USER_1).expect("固定卖家"));
    let listing = Uuid::parse_str(LISTING).expect("代管过的挂单");
    let page = runtime.list_owned(seller, query()).await.expect("旧列表");
    assert_eq!((page.total, page.items.len()), (0, 0));
    assert!(runtime.owned_detail(seller, listing).await.expect("详情").is_none());
    assert!(runtime.admin_detail(listing).await.expect("Admin 详情").is_none());
    let admin = runtime
        .list_admin(AdminListingQuery { listings: query(), seller_user_id: None })
        .await
        .expect("Admin 旧列表");
    assert_eq!((admin.total, admin.items.len()), (0, 0));
}
