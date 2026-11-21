//! 通过写端口验证已证返还历史、登记基线与解析再次核验。

use auction_application::consignment::{ListingBusinessRejection, ListingCommandStore};
use auction_domain::consignment::ListingMode;
use sqlx::{AssertSqlSafe, PgConnection};
use uuid::Uuid;

use super::fixture::{Fixture, OTHER, SELLER, apply, gear, new_id, observation, register};

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
                new_id(),
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
