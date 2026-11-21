//! 验证 PG18 在历史数据规模下使用寄售迁移提供的索引。

use std::io::{Write, stdout};

use sqlx::{AssertSqlSafe, PgConnection};
use uuid::Uuid;

use super::fixtures::{END, Listing, SELLER, at, auction, batch, commit, equip, seed};
use super::production::history_query::HISTORY_SQL;
use super::production::lots::{LISTING_COLLISION_SQL, insert_registration_baselines};
use super::production::occupancy::{ACTIVE_COUNT_SQL, HOURLY_COUNT_SQL};
use crate::support::TestDatabase;

fn explain(sql: &str) -> AssertSqlSafe<String> {
    AssertSqlSafe(format!("EXPLAIN (ANALYZE, BUFFERS) {sql}"))
}

async fn check_plans(cx: &mut PgConnection, listing_id: Uuid) {
    let samples = [
        (
            "collision",
            sqlx::query_scalar::<_, String>(explain(LISTING_COLLISION_SQL))
                .bind(listing_id)
                .fetch_all(&mut *cx)
                .await
                .expect("碰撞执行计划"),
        ),
        (
            "history",
            sqlx::query_scalar::<_, String>(explain(HISTORY_SQL))
                .bind("main")
                .bind("42")
                .fetch_all(&mut *cx)
                .await
                .expect("历史执行计划"),
        ),
        (
            "hourly",
            sqlx::query_scalar::<_, String>(explain(HOURLY_COUNT_SQL))
                .bind(SELLER)
                .bind(at(END))
                .fetch_all(&mut *cx)
                .await
                .expect("小时额度执行计划"),
        ),
        (
            "activity",
            sqlx::query_scalar::<_, String>(explain(ACTIVE_COUNT_SQL))
                .bind(SELLER)
                .bind(at(END))
                .fetch_all(&mut *cx)
                .await
                .expect("活动额度执行计划"),
        ),
    ];
    let mut missing = Vec::new();
    let mut buffers = Vec::new();
    for (name, rows) in samples {
        let root_buffers = rows.iter().find(|row| row.contains("Buffers:"));
        buffers.push((name, root_buffers.expect("计划缺少缓冲区计数").trim().to_owned()));
        // 便于本地查看脱敏计划。断言失败另外附上根节点的缓冲页。
        let plan =
            rows.into_iter().filter(|row| !row.contains("never executed")).collect::<Vec<_>>();
        writeln!(stdout().lock(), "\n2E PG18 {name}\n{}", plan.join("\n")).expect("输出测试计划");
        if name == "hourly" {
            continue;
        }
        for index in ["equipment_consignment_eid_alias_idx", "lots_equipment_id_idx"] {
            let used = plan.iter().any(|row| {
                row.split_ascii_whitespace().any(|word| word == index)
                    && (row.contains("Index Scan") || row.contains("Index Only Scan"))
            });
            if !used {
                missing.push((name, index));
            }
        }
    }
    assert!(missing.is_empty(), "未执行的寄售索引扫描：{missing:?}；缓冲页：{buffers:?}");
}

#[tokio::test]
async fn postgres_explain_access_paths() {
    let db = TestDatabase::create().await.expect("临时 PG18");
    let owner = db.maintenance_store().await.expect("维护连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let mut tx = pool.begin().await.expect("性能数据");
    let equipment_id = equip(&mut tx, "main", "00042").await;
    auction(&mut tx, "main", 1, "settled").await;
    batch(&mut tx, &format!(r#"
        INSERT INTO public.equipment (id,world_id,external_equipment_id,name,category,collected_at)
        SELECT gen_random_uuid(),'main',(100000+n)::text,'Fake Equipment','Weapon','2026-01-01'
        FROM generate_series(1,20000) n;
        INSERT INTO public.auctions
            (id,world_id,sequence,title,status,scheduled_open_at,preview_scheduled_at,policy_snapshot)
        SELECT gen_random_uuid(),'main',n,'Fake Auction','settled','2026-01-01','2026-01-01',
               (SELECT policy_snapshot FROM public.auctions WHERE world_id='main' AND sequence=1)
        FROM generate_series(2,1501) n;
        INSERT INTO public.lots
            (id,auction_id,equipment_id,lot_number,status,start_price,category_key,category_number)
        SELECT gen_random_uuid(),id,'{equipment_id}',1,'unsold',50000,'Weapon',1
        FROM public.auctions WHERE sequence <= 1500;
        INSERT INTO public.lots
            (id,auction_id,equipment_id,lot_number,status,start_price,category_key,category_number)
        SELECT gen_random_uuid(),a.id,e.id,row_number() OVER ()::integer,'unsold',50000,'Weapon',
               row_number() OVER ()::integer
        FROM public.auctions a CROSS JOIN public.equipment e
        WHERE a.sequence=1501 AND e.id <> '{equipment_id}';
        ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
        INSERT INTO public.consignment_listings
            (id,seller_user_id,world_id,external_equipment_id,mode,state,price,pricing_version,fee_bps,
             state_version,service_epoch,blacklist_count_at_register,blacklist_baseline_ids,
             reason_code,created_at,updated_at,expires_at,
             registration_xact_id)
        SELECT gen_random_uuid(),'{SELLER}','main',(300000+n)::text,'direct','closed',101,1,0,2,1,0,ARRAY[]::uuid[],
               'seller_cancelled','2025-12-01'::timestamptz + n*interval '1 second',
               '2025-12-02','2025-12-03',pg_current_xact_id()
        FROM generate_series(1,20000) n;
        SET CONSTRAINTS ALL IMMEDIATE;
        ALTER TABLE public.consignment_listings ENABLE TRIGGER USER;
    "#)).await;
    commit(tx).await;
    // 活动额度也要扫过这件有大量基线 lot 的装备。
    let listing = Listing { created: "2026-01-01T22:00:00Z", ..Listing::new(101, "42") };
    let mut tx = pool.begin().await.expect("性能夹具基线");
    listing.register(&mut tx).await;
    assert_eq!(
        insert_registration_baselines(&mut tx, listing.listing_id).await.expect("历史基线"),
        1500
    );
    for (listing_id, eid, world) in
        [(102, "43", "isekai"), (103, "44", "main"), (104, "45", "main")]
    {
        Listing { created: END, world, ..Listing::new(listing_id, eid) }.register(&mut tx).await;
    }
    commit(tx).await;

    let mut cx = pool.acquire().await.expect("计划连接");
    let pg_version: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&mut *cx)
        .await
        .expect("测试服务器版本");
    assert!(pg_version.starts_with("18"));
    batch(&mut cx, "ANALYZE;").await;
    check_plans(&mut cx, listing.listing_id).await;
}
