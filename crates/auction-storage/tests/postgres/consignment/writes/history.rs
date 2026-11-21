//! 通过写端口验证已证返还历史、登记基线与解析再次核验。

use std::sync::Arc;
use std::time::Duration;

use auction_application::auction::{CompetitionCommandPort, PlaceBidCommand};
use auction_application::consignment::{ListingBusinessRejection, ListingCommandStore};
use auction_domain::consignment::ListingMode;
use auction_domain::{BidAmount, CommandId, LotId, UserId};
use auction_storage::DatabaseCompetitionCommands;
use auction_time::{Duration as TimeDelta, Timestamp};
use sqlx::{AssertSqlSafe, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::fixture::{
    Fixture, OTHER, SELLER, apply, gear, new_id, new_internal_command_id, observation, register,
};
use crate::support::fixtures::{policy_snapshot, seed_user, wait_for_condition};

#[tokio::test]
async fn postgres_history_verified_alias_freezes_baselines_for_both_modes_and_is_rechecked() {
    let f = Fixture::new().await;
    for (eid, mode, sequence) in
        [("461", ListingMode::Direct, 1), ("462", ListingMode::Consigned, 2)]
    {
        let alias = format!("{}{}", "0".repeat(61), eid);
        let listing_id = new_id();
        let lot_id = f.lot(&alias, sequence).await;
        let equipment = gear("main", eid);
        let attempt = |command_id, seller| {
            f.store.register(
                register(command_id, listing_id, &equipment, &f.key, seller, mode),
                Some(&f.crypto),
                Some(&f.crypto),
            )
        };
        assert_eq!(
            attempt(new_id(), SELLER).await.unwrap().into_result(),
            Err(ListingBusinessRejection::EquipmentBusy.into())
        );

        let mut tx = f.owner.postgres_pool().begin().await.unwrap();
        record_collected_return(&mut tx, lot_id).await;
        tx.commit().await.unwrap();
        assert_eq!(
            attempt(new_id(), OTHER).await.unwrap().into_result(),
            Err(ListingBusinessRejection::HistoryOwnerMismatch.into())
        );
        let receipt = attempt(new_id(), SELLER).await.unwrap().into_result().unwrap();
        assert_eq!(receipt.listing_id(), listing_id);
        let baseline: Vec<Uuid> = sqlx::query_scalar(
            "SELECT lot_id FROM public.consignment_listing_lot_baselines WHERE listing_id=$1",
        )
        .bind(listing_id)
        .fetch_all(f.pool())
        .await
        .unwrap();
        assert_eq!(baseline, [lot_id]);

        let work_id =
            sqlx::query_scalar("SELECT id FROM public.consignment_lookup_work WHERE listing_id=$1")
                .bind(listing_id)
                .fetch_one(f.pool())
                .await
                .unwrap();
        let fence = f.grant(listing_id, work_id, 1, 30).await;
        if mode == ListingMode::Consigned {
            // 新孤立别名不产生 lot 碰撞，但会新增历史证据不足。发布必须重查历史门。
            sqlx::query(
                "INSERT INTO public.equipment(id,world_id,external_equipment_id,name,category,collected_at)
                 VALUES (gen_random_uuid(),'main',$1,'Fake Equipment','Weapon',statement_timestamp())",
            ).bind(format!("00{eid}")).execute(f.owner.postgres_pool()).await.unwrap();
        }
        f.store
            .apply_lookup(apply(
                new_internal_command_id(),
                fence,
                observation(&equipment, &f.key, "FakeWriteSeller", true),
                1,
            ))
            .await
            .unwrap()
            .into_result()
            .unwrap();
        if mode == ListingMode::Direct {
            assert_eq!(f.state(listing_id).await, ("displayed".into(), 2, None, true));
        } else {
            assert_eq!(
                f.state(listing_id).await,
                ("rejected".into(), 2, Some("history_unproven".into()), true)
            );
        }
        assert!(f.retired(listing_id).await);
    }
}

async fn record_collected_return(tx: &mut PgConnection, lot_id: Uuid) {
    let (auction_id, equipment_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT auction_id,equipment_id FROM public.lots WHERE id=$1")
            .bind(lot_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let submission_id = new_id();
    let intake_id = new_id();
    let settlement_id = new_id();
    let obligation_id = new_id();
    let delivery_id = new_id();
    let delivery_command_id = new_id();
    // UUID 来自夹具，所有字面量均为脱敏的拍卖物权证据。
    sqlx::raw_sql(AssertSqlSafe(format!(r#"
        INSERT INTO public.auction_commands (command_id,command_kind,aggregate_id,request_hash,result_code)
        VALUES ('{intake_id}','fake_intake','{auction_id}',decode(repeat('11',32),'hex'),'accepted'),
               ('{settlement_id}','fake_settlement','{auction_id}',decode(repeat('11',32),'hex'),'accepted');
        INSERT INTO public.auction_submissions
            (id,source_command_id,auction_id,seller,received_at,source_kind,source_reference,source_fingerprint)
        VALUES ('{submission_id}','{intake_id}','{auction_id}','FakeWriteSeller','2026-01-02T00:00:00Z',
                'admin','{submission_id}',decode(replace('{submission_id}','-','') || replace('{intake_id}','-',''),'hex'));
        UPDATE public.lots SET submission_id='{submission_id}' WHERE id='{lot_id}';
        INSERT INTO public.settlement_obligations
            (id,auction_id,lot_id,world_id,origin_command_id,policy_version,obligation_kind,
             business_state,recipient_game_identity,equipment_id,submission_id)
        VALUES ('{obligation_id}','{auction_id}','{lot_id}','main','{settlement_id}',1,'unsold_return',
                'ready','FakeWriteSeller','{equipment_id}','{submission_id}');
        INSERT INTO public.deliveries
            (id,command_id,request_digest,origin_kind,origin_id,recipient,payload,status,
             external_reference,result_metadata,completed_at)
        VALUES ('{delivery_id}','{delivery_command_id}',decode(repeat('11',32),'hex'),
                'settlement','{obligation_id}','FakeWriteSeller',
                '{{"schema_version":1,"world":"main","operation":"send",
                   "equipment_secret_refs":[{{"kind":"auction_equipment_key","id":"{equipment_id}"}}]}}',
                'succeeded','mooglemail:main:send:12345',
                '{{"external_reference_proven":true,"operation":"send","proof":"sent_exact_payload_v1"}}',
                '2026-01-02T01:00:00Z');
        INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id,execution_state,dispatch_world,delivery_operation,delivery_id,
             completed_at,proof_kind,proof_reference,proof_digest,proof_observed_at)
        VALUES ('{obligation_id}','satisfied','main','send','{delivery_id}',
                '2026-01-02T02:00:00Z','mooglemail_sent_collected','12345',
                decode(repeat('11',32),'hex'),'2026-01-02T02:00:00Z');
    "#))).execute(tx).await.unwrap();
}

const REGISTRATION_GATE: i64 = 802001;
const BID_GATE: i64 = 802002;

async fn install_transaction_gates(pool: &PgPool) {
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_transaction_gate() RETURNS trigger
           LANGUAGE plpgsql AS $$
         BEGIN
           PERFORM pg_advisory_xact_lock(TG_ARGV[0]::bigint);
           RETURN NEW;
         END $$;
         CREATE TRIGGER fake_consignment_registration_gate BEFORE INSERT ON public.consignment_listings
           FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_transaction_gate('802001');
         CREATE TRIGGER fake_consignment_bid_gate BEFORE INSERT ON public.bids
           FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_transaction_gate('802002');",
    ).execute(pool).await.unwrap();
}

async fn hold_gate(pool: &PgPool, gate: i64) -> Transaction<'_, Postgres> {
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(gate)
        .execute(&mut *blocker)
        .await
        .unwrap();
    blocker
}

async fn wait_at_gate(pool: &PgPool, gate: i64) {
    assert!(wait_for_condition(Duration::from_secs(3), || async {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_locks
             WHERE locktype='advisory' AND NOT granted AND classid=0 AND objsubid=1
               AND objid=$1::bigint::oid AND database=(SELECT oid FROM pg_database WHERE datname=current_database()))"
        ).bind(gate).fetch_one(pool).await.unwrap()
    }).await, "事务须到达显式同步点");
}

#[tokio::test]
async fn postgres_history_lot_committed_after_registration_snapshot_remains_outside_baseline() {
    let f = Fixture::new().await;
    let pool = f.owner.postgres_pool();
    install_transaction_gates(pool).await;
    let g = gear("main", "463");
    let listing_id = new_id();
    let blocker = hold_gate(pool, REGISTRATION_GATE).await;

    let registration = f.registered(listing_id, &g, SELLER, ListingMode::Consigned);
    let insert_lot = async {
        // 挂单 INSERT 之前的资格与装备历史查询已固定 Serializable 快照。
        wait_at_gate(pool, REGISTRATION_GATE).await;
        let late = f.lot("463", 1).await;
        blocker.commit().await.unwrap();
        late
    };
    let (work_id, late) = tokio::join!(registration, insert_lot);
    let baseline: Vec<Uuid> = sqlx::query_scalar(
        "SELECT lot_id FROM public.consignment_listing_lot_baselines WHERE listing_id=$1",
    )
    .bind(listing_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(baseline.is_empty());
    let fence = f.grant(listing_id, work_id, 1, 30).await;
    let applied = f
        .store
        .apply_lookup(apply(
            new_internal_command_id(),
            fence,
            observation(&g, &f.key, "FakeWriteSeller", true),
            1,
        ))
        .await
        .unwrap()
        .into_result();
    assert_eq!(applied, Err(ListingBusinessRejection::ListingNotActive.into()));
    assert_eq!(
        f.state(listing_id).await,
        ("closed".into(), 2, Some("auction_lot_created".into()), false)
    );
    let conflicts: Vec<Uuid> = sqlx::query_scalar(
        "SELECT lot_id FROM public.consignment_listing_auction_conflicts WHERE listing_id=$1",
    )
    .bind(listing_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(conflicts, [late]);
}

async fn make_live_fixture(f: &Fixture, lot_id: Uuid) {
    let mut tx = f.owner.postgres_pool().begin().await.unwrap();
    let opened = Timestamp::now() - TimeDelta::minutes(30);
    let deadline = opened + TimeDelta::hours(2);
    let auction_id: Uuid = sqlx::query_scalar(
        "UPDATE public.auctions SET status='live', scheduled_open_at=$2, preview_scheduled_at=$2,
            preview_opened_at=$2, opened_at=$2, scheduled_end_at=$3, effective_end_at=$3,
            policy_snapshot=$4 WHERE id=(SELECT auction_id FROM public.lots WHERE id=$1)
         RETURNING id",
    )
    .bind(lot_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy_snapshot(deadline, 50_000, 30_000))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE public.lots SET status='live',current_price=50000,effective_end_at=$2 WHERE id=$1",
    )
    .bind(lot_id)
    .bind(deadline)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO public.realtime_streams (stream_key,next_sequence) VALUES ($1,1)")
        .bind(format!("auction:{auction_id}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn postgres_history_registration_and_live_bid_do_not_block_each_other() {
    let f = Fixture::new().await;
    let pool = f.owner.postgres_pool();
    install_transaction_gates(pool).await;
    let lot = f.lot("464", 1).await;
    make_live_fixture(&f, lot).await;
    let third_player = new_id();
    seed_user(pool, third_player, "FakeThirdBidder").await;
    let bids = DatabaseCompetitionCommands::new(Arc::new(f.store.clone()));
    let bid = |credits, bidder| {
        let lot_id = LotId::from_uuid(lot);
        let bidder_id = UserId::from_uuid(bidder);
        let amount = BidAmount::new(credits).unwrap();
        PlaceBidCommand {
            command_id: CommandId::from_uuid(new_id()),
            lot_id,
            bidder_id,
            amount,
            request_hash: PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount).unwrap(),
            request_id: None,
        }
    };

    let first_gear = gear("main", "465");
    let held_bid = hold_gate(pool, BID_GATE).await;
    let (first, ()) = tokio::join!(bids.place_bid(bid(80_000, OTHER)), async {
        // 正常竞价已经拿到 FOR SHARE OF world，且须等登记提交才会释放。
        wait_at_gate(pool, BID_GATE).await;
        f.registered(new_id(), &first_gear, SELLER, ListingMode::Direct).await;
        held_bid.commit().await.unwrap();
    });
    first.expect("登记提交后竞价仍须完成");

    let second_gear = gear("main", "466");
    let held_registration = hold_gate(pool, REGISTRATION_GATE).await;
    let (_, ()) =
        tokio::join!(f.registered(new_id(), &second_gear, SELLER, ListingMode::Direct), async {
            wait_at_gate(pool, REGISTRATION_GATE).await;
            bids.place_bid(bid(110_000, third_player)).await.expect("登记阻塞时竞价仍须提交");
            held_registration.commit().await.unwrap();
        },);
    let committed: Vec<i64> =
        sqlx::query_scalar("SELECT amount FROM public.bids WHERE lot_id=$1 ORDER BY amount")
            .bind(lot)
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(committed, [80_000, 110_000]);
}
