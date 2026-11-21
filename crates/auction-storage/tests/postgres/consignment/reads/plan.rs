//! 测量生产分页 SQL 在较大历史库中的访问路径。

use std::io::{Write, stdout};

use auction_application::consignment::{AdminListingQuery, ListingWorld, PublicListingQuery};
use auction_domain::consignment::ListingMode;

use super::fixture::{Fixture, SELLER, batch, private_query, public_query, seller};

#[tokio::test]
async fn postgres_read_access_paths_with_large_history() {
    let fixture = Fixture::new().await;
    fixture.lot("main", "00042", 1).await;
    let mut tx = fixture.pool().begin().await.unwrap();
    batch(&mut tx, &format!(r#"
        INSERT INTO public.equipment (id,world_id,external_equipment_id,name,category,collected_at)
        SELECT gen_random_uuid(),'main',(100000+n)::text,'Fake Equipment','Weapon',statement_timestamp()
        FROM generate_series(1,20000) n;
        INSERT INTO public.lots
            (id,auction_id,equipment_id,lot_number,status,start_price,category_key,category_number)
        SELECT gen_random_uuid(),a.id,e.id,row_number() OVER ()::integer + 1,'unsold',50000,'Weapon',
               row_number() OVER ()::integer + 1
        FROM public.auctions a CROSS JOIN public.equipment e
        WHERE a.sequence=1 AND e.external_equipment_id <> '00042';

        INSERT INTO public.users(id,username,public_key_hash)
        SELECT md5('p2r-seller-' || n)::uuid, 'Fake Plan Seller ' || n,
               decode(repeat(md5('p2r-key-' || n), 2), 'hex')
        FROM generate_series(1,99) n;

        -- 仅计划用的合成历史，隔离的夹具库内仍保留行形状和唯一约束。
        ALTER TABLE public.consignment_listings DISABLE TRIGGER USER;
        INSERT INTO public.consignment_listings
            (id,seller_user_id,world_id,external_equipment_id,mode,state,price,pricing_version,fee_bps,
             state_version,service_epoch,blacklist_count_at_register,reason_code,created_at,updated_at,expires_at,
             registration_xact_id)
        SELECT gen_random_uuid(),CASE WHEN n <= 300 THEN '{SELLER}'::uuid
                   ELSE md5('p2r-seller-' || ((n-1)/300))::uuid END,
               CASE WHEN n <= 400 THEN 'main' ELSE 'isekai' END,
               CASE WHEN n = 1 THEN '42' ELSE (n+50000)::text END,
               CASE WHEN n <= 400 THEN 'direct' ELSE 'consigned' END,
               'closed',101,1,CASE WHEN n <= 400 THEN 0 ELSE 500 END,2,1,0,'seller_cancelled',
               statement_timestamp() - interval '1 day',
               statement_timestamp() - n * interval '1 second',
               statement_timestamp() + interval '1 hour', pg_current_xact_id()
        FROM generate_series(1,30000) n;
        INSERT INTO public.consignment_snapshots
            (id,listing_id,generation,adapter,observed_at,digest,world_id,external_equipment_id,
             name,category,level_kind,level_number,tradeable,owner_name,stat_groups)
        SELECT gen_random_uuid(),l.id,1,'hvdata-equip-v1',l.updated_at,decode(repeat('11',32),'hex'),
               l.world_id,l.external_equipment_id,'Fake Equipment','Weapon','assigned',100,true,'FakeSeller','[]'
        FROM public.consignment_listings l WHERE l.world_id='main';
        UPDATE public.consignment_listings l
        SET state='displayed',reason_code=NULL,current_snapshot_id=s.id
        FROM public.consignment_snapshots s WHERE s.listing_id=l.id;
        SET CONSTRAINTS ALL IMMEDIATE;
        ALTER TABLE public.consignment_listings ENABLE TRIGGER USER;
    "#)).await;
    tx.commit().await.unwrap();
    fixture.sql("ANALYZE").await;
    let config = PublicListingQuery {
        world: Some(ListingWorld::Main),
        mode: Some(ListingMode::Direct),
        ..public_query()
    };
    let admin = AdminListingQuery { seller_user_id: Some(seller()), listings: private_query() };
    let plans = fixture.store.test_consignment_read_plans(&config, admin).await.unwrap();
    for (name, plan) in &plans {
        writeln!(stdout().lock(), "\n2R PG18 {name}\n{}", plan.join("\n")).unwrap();
        assert!(plan.iter().any(|row| row.contains("Buffers: shared")));
    }
    let public_count = &plans[1].1;
    for index in ["equipment_consignment_eid_alias_idx", "lots_equipment_id_idx"] {
        assert!(
            public_count.iter().any(|row| row.contains(index) && !row.contains("never executed")),
            "碰撞检查缺少索引 {index}"
        );
    }
    let owned_page =
        plans.iter().find(|(name, _)| *name == "owned_page").map(|(_, rows)| rows).unwrap();
    assert!(owned_page.iter().any(|row| row.contains("consignment_listings_seller_idx")));
}
