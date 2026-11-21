//! 在独立的迁移测试库构造拍卖和寄售事实。

use auction_domain::consignment::ListingEquipmentIdentity;
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgConnection, PgPool};
use uuid::Uuid;

use crate::support;

pub(super) const SELLER: Uuid = Uuid::from_u128(100);
pub(super) const OTHER: Uuid = Uuid::from_u128(200);
pub(super) const REPLAY: Uuid = Uuid::from_u128(300);
pub(super) const START: &str = "2026-01-01T00:00:00Z";
pub(super) const LATER: &str = "2026-01-01T01:00:00Z";
pub(super) const END: &str = "2026-01-02T00:00:00Z";

pub(super) fn at(value: &str) -> Timestamp {
    value.parse().expect("测试时刻")
}

pub(super) fn gear(world: &str, eid: &str) -> ListingEquipmentIdentity {
    ListingEquipmentIdentity::new(world, eid).expect("测试装备身份")
}

pub(super) async fn seed(pool: &PgPool) {
    support::fixtures::seed_user(pool, SELLER, "FakeSeller").await;
    support::fixtures::seed_user(pool, OTHER, "FakeOther").await;
    batch(
        &mut pool.acquire().await.expect("测试连接"),
        &format!(
            "UPDATE public.consignment_lookup_control SET admitting = true;
         INSERT INTO public.consignment_replay_keys
             (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('{REPLAY}', true, decode(repeat('11',48),'hex'), decode(repeat('22',12),'hex'),
                 'fake-data-key', '{START}', '{START}');"
        ),
    )
    .await;
}

pub(super) async fn batch(tx: &mut PgConnection, sql: &str) {
    if let Err(error) = sqlx::raw_sql(AssertSqlSafe(sql.to_owned())).execute(tx).await {
        let database = error.as_database_error();
        panic!(
            "测试 SQL 失败：SQLSTATE={:?}，约束={:?}，消息={}",
            database.and_then(sqlx::error::DatabaseError::code),
            database.and_then(sqlx::error::DatabaseError::constraint),
            database.map_or("读取或连接失败", sqlx::error::DatabaseError::message)
        );
    }
}

pub(super) async fn commit(tx: sqlx::Transaction<'_, sqlx::Postgres>) {
    if let Err(error) = tx.commit().await {
        let database = error.as_database_error();
        panic!(
            "测试提交失败：SQLSTATE={:?}，约束={:?}，消息={}",
            database.and_then(sqlx::error::DatabaseError::code),
            database.and_then(sqlx::error::DatabaseError::constraint),
            database.map_or("连接失败", sqlx::error::DatabaseError::message)
        );
    }
}

pub(super) struct Listing<'a> {
    pub(super) listing_id: Uuid,
    pub(super) seller: Uuid,
    pub(super) world: &'a str,
    pub(super) eid: &'a str,
    pub(super) mode: &'a str,
    pub(super) created: &'a str,
    pub(super) expires: &'a str,
    pub(super) epoch: i64,
    pub(super) blacklist_count: i64,
}

impl<'a> Listing<'a> {
    pub(super) fn new(listing_id: u128, eid: &'a str) -> Self {
        Self {
            listing_id: Uuid::from_u128(listing_id),
            seller: SELLER,
            world: "main",
            eid,
            mode: "direct",
            created: START,
            expires: END,
            epoch: 1,
            blacklist_count: 0,
        }
    }

    pub(super) async fn register(&self, tx: &mut PgConnection) {
        let Self { listing_id, seller, world, eid, mode, created, epoch, blacklist_count, .. } =
            self;
        let deadline: Timestamp = at(created) + auction_time::Duration::hours(24);
        let command_id = Uuid::new_v4();
        let fee = if *mode == "direct" { 0 } else { 500 };
        batch(tx, &format!(r#"
            INSERT INTO public.consignment_listings
                (id,seller_user_id,world_id,external_equipment_id,mode,state,price,pricing_version,
                 fee_bps,state_version,service_epoch,blacklist_count_at_register,created_at,updated_at,expires_at)
            VALUES ('{listing_id}','{seller}','{world}','{eid}','{mode}','resolving',101,1,
                    {fee},1,{epoch},{blacklist_count},'{created}','{created}','{deadline}');
            INSERT INTO public.consignment_commands
                (command_id,actor_kind,actor_ref,operation,request_target,request_digest,replay_key_id,
                 listing_id,result_status,result_code,result_body,created_at)
            VALUES ('{command_id}','public_user','{seller}','register','new',decode(repeat('11',32),'hex'),
                    '{REPLAY}','{listing_id}',202,'accepted',
                    '{{"command_id":"{command_id}","operation":"register","listing_id":"{listing_id}",
                       "listing_version":1,"state":"resolving","disposition":"registered","price":101}}',
                    '{created}');
            INSERT INTO public.consignment_events
                (listing_id,state_version,command_id,actor_kind,event_kind,to_state,price,occurred_at)
            VALUES ('{listing_id}',1,'{command_id}','public_user','registered','resolving',101,'{created}');
            "#)).await;
    }

    pub(super) async fn publish(&self, tx: &mut PgConnection) {
        let Self { listing_id, world, eid, mode, expires, .. } = self;
        let snapshot_id = Uuid::new_v4();
        let command_id = Uuid::new_v4();
        let state = if *mode == "direct" { "displayed" } else { "awaiting_deposit" };
        batch(tx, &format!(r#"
            INSERT INTO public.consignment_snapshots
                (id,listing_id,generation,adapter,observed_at,digest,world_id,external_equipment_id,
                 name,category,level_kind,level_number,tradeable,owner_name,stat_groups)
            VALUES ('{snapshot_id}','{listing_id}',1,'hvdata-equip-v1','{LATER}',decode(repeat('11',32),'hex'),
                    '{world}','{eid}','Fake Equipment','Weapon','assigned',100,true,'FakeSeller','[]');
            UPDATE public.consignment_listings SET state = '{state}', state_version = 2,
                current_snapshot_id = '{snapshot_id}', updated_at = '{LATER}', expires_at = '{expires}'
            WHERE id = '{listing_id}';
            INSERT INTO public.consignment_commands
                (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
                 listing_id,result_status,result_code,result_body,created_at)
            VALUES ('{command_id}','worker','fake-worker','apply_lookup','{listing_id}',decode(repeat('22',32),'hex'),
                    '{listing_id}',200,'ok',
                    '{{"command_id":"{command_id}","operation":"apply_lookup","listing_id":"{listing_id}",
                       "listing_version":2,"state":"{state}","disposition":"published","price":101,
                       "expires_at":"{expires}","reason_code":null}}','{LATER}');
            INSERT INTO public.consignment_events
                (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,price,occurred_at)
            VALUES ('{listing_id}',2,'{command_id}','worker','published','resolving','{state}',101,'{LATER}');
        "#)).await;
    }

    pub(super) async fn withdraw_resolving(&self, tx: &mut PgConnection, closed_at: &str) {
        let Self { listing_id, seller, .. } = self;
        let command_id = Uuid::new_v4();
        batch(tx, &format!(r#"
            UPDATE public.consignment_listings SET state='closed',state_version=2,
                reason_code='seller_cancelled',updated_at='{closed_at}' WHERE id='{listing_id}';
            INSERT INTO public.consignment_commands
                (command_id,actor_kind,actor_ref,operation,request_target,request_digest,listing_id,
                 result_status,result_code,result_body,created_at)
            VALUES ('{command_id}','public_user','{seller}','withdraw','{listing_id}',
                    decode(repeat('22',32),'hex'),'{listing_id}',200,'ok',
                    '{{"command_id":"{command_id}","operation":"withdraw","listing_id":"{listing_id}",
                       "listing_version":2,"state":"closed","disposition":"closed","price":101}}','{closed_at}');
            INSERT INTO public.consignment_events
                (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,reason_code,price,occurred_at)
            VALUES ('{listing_id}',2,'{command_id}','public_user','closed','resolving','closed','seller_cancelled',101,'{closed_at}');
        "#)).await;
    }
}

pub(super) async fn equip(tx: &mut PgConnection, world: &str, eid: &str) -> Uuid {
    let equipment_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.equipment(id,world_id,external_equipment_id,name,category,collected_at)
         VALUES($1,$2,$3,'Fake Equipment','Weapon',$4)",
    )
    .bind(equipment_id)
    .bind(world)
    .bind(eid)
    .bind(at(START))
    .execute(tx)
    .await
    .expect("写入测试装备");
    equipment_id
}

pub(super) async fn auction(
    tx: &mut PgConnection,
    world: &str,
    sequence: i64,
    status: &str,
) -> Uuid {
    let auction_id = Uuid::new_v4();
    let deadline = at(END);
    sqlx::query(
        "INSERT INTO public.auctions
           (id,world_id,sequence,title,status,scheduled_open_at,preview_scheduled_at,scheduled_end_at,
            effective_end_at,preview_opened_at,opened_at,policy_snapshot)
         VALUES ($1,$2,$3,'Fake Auction',$4,$5,$5,$5,$5,$5,$5,$6)"
    )
    .bind(auction_id).bind(world).bind(sequence).bind(status).bind(deadline)
    .bind(support::fixtures::policy_snapshot(deadline, 50_000, 30_000))
    .execute(&mut *tx).await.expect("写入测试场次");
    if status == "draft" {
        sqlx::query(
            "INSERT INTO public.auction_intake_targets (world_id,auction_id,target_status)
             VALUES ($1,$2,'draft')",
        )
        .bind(world)
        .bind(auction_id)
        .execute(tx)
        .await
        .expect("写入草稿收件目标");
    }
    auction_id
}

pub(super) async fn lot(
    tx: &mut PgConnection,
    lot_id: Uuid,
    auction_id: Uuid,
    equipment_id: Uuid,
    number: i32,
    status: &str,
    submission_id: Option<Uuid>,
) {
    let submission_id = if status == "pending" && submission_id.is_none() {
        Some(submission(tx, auction_id, "admin", "FakeSeller", START).await)
    } else {
        submission_id
    };
    sqlx::query(
        "INSERT INTO public.lots
            (id,auction_id,equipment_id,lot_number,status,start_price,category_key,category_number,
             submission_id,created_at,updated_at,effective_end_at)
         VALUES ($1,$2,$3,$4,$5,50000,'Weapon',$4,$6,$7,$7,
                 CASE WHEN $5 IN ('live','sold') THEN $7::timestamptz END)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(number)
    .bind(status)
    .bind(submission_id)
    .bind(at(START))
    .execute(tx)
    .await
    .expect("写入测试 lot");
}

pub(super) async fn submission(
    tx: &mut PgConnection,
    auction_id: Uuid,
    source: &str,
    seller: &str,
    received_at: &str,
) -> Uuid {
    let submission_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let mailbox_key = (source == "mail").then_some("fake-mailbox");
    let external_mail_id = (source == "mail").then_some("12000");
    batch(tx, &format!("INSERT INTO public.auction_commands
        (command_id,command_kind,aggregate_id,request_hash,result_code)
         VALUES ('{command_id}','fake_intake','{auction_id}',decode(repeat('11',32),'hex'),'accepted');"
    )).await;
    sqlx::query(
        "INSERT INTO public.auction_submissions
           (id,source_command_id,auction_id,mailbox_key,external_mail_id,seller,received_at,
            source_kind,source_reference,source_fingerprint)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,
                 CASE WHEN $8='admin' THEN $1::uuid::text END,
                 CASE WHEN $8='admin' THEN decode(
                    replace($1::uuid::text,'-','') || replace($2::uuid::text,'-',''),'hex'
                 ) END)",
    )
    .bind(submission_id)
    .bind(command_id)
    .bind(auction_id)
    .bind(mailbox_key)
    .bind(external_mail_id)
    .bind(seller)
    .bind(at(received_at))
    .bind(source)
    .execute(tx)
    .await
    .expect("写入测试投稿");
    submission_id
}
