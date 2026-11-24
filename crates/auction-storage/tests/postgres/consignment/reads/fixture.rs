//! 构造符合迁移约束的寄售读侧事实。夹具时刻以数据库为准。

use auction_application::consignment::{
    ListingPageWindow, PrivateListingQuery, PublicListingQuery,
};
use auction_domain::UserId;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::{AssertSqlSafe, PgConnection, PgPool};
use uuid::Uuid;

use crate::support::{TestDatabase, fixtures};

pub(super) const SELLER: Uuid = Uuid::from_u128(100);
pub(super) const OTHER: Uuid = Uuid::from_u128(200);
pub(super) const REPLAY: Uuid = Uuid::from_u128(300);
pub(super) const HISTORY: Uuid = Uuid::from_u128(400);

pub(super) struct Fixture {
    pub(super) store: DatabaseStore,
    owner: DatabaseStore,
    pub(super) now: Timestamp,
    _db: TestDatabase,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        Self::create(true).await
    }

    pub(super) async fn create(admit: bool) -> Self {
        let db = TestDatabase::create().await.expect("临时 PG18");
        let owner = db.maintenance_store().await.expect("测试管理连接");
        let pool = owner.postgres_pool();
        fixtures::seed_user(pool, SELLER, "FakeSeller").await;
        fixtures::seed_user(pool, OTHER, "FakeOther").await;
        let now: Timestamp = sqlx::query_scalar("SELECT date_trunc('second', clock_timestamp())")
            .fetch_one(pool)
            .await
            .expect("测试数据库时刻");
        let store = db.store().await.expect("测试运行连接");
        let fixture = Self { store, owner, now, _db: db };
        if admit {
            fixture.sql("UPDATE public.consignment_lookup_control SET admitting = true").await;
        }
        fixture.seed_replay().await;
        fixture
    }

    pub(super) async fn seed_replay(&self) {
        self.sql(&format!(
            "INSERT INTO public.consignment_replay_keys
                (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ('{REPLAY}', true, decode(repeat('11',48),'hex'), decode(repeat('22',12),'hex'),
                     'fake-data-key', '{0}', '{0}')",
            self.now
        ))
        .await;
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    pub(super) async fn sql(&self, sql: &str) {
        batch(&mut self.pool().acquire().await.expect("管理连接"), sql).await;
    }

    pub(super) async fn lot(&self, world: &str, eid: &str, sequence: i64) -> Uuid {
        let mut tx = self.pool().begin().await.expect("测试 lot 事务");
        let equipment: Uuid = sqlx::query_scalar(
            "INSERT INTO public.equipment (id, world_id, external_equipment_id, name, category, collected_at)
             VALUES (gen_random_uuid(), $1, $2, 'Fake Equipment', 'Weapon', $3)
             ON CONFLICT (world_id, external_equipment_id) DO UPDATE SET name = equipment.name
             RETURNING id"
        ).bind(world).bind(eid).bind(self.now).fetch_one(&mut *tx).await.expect("测试装备");
        let auction_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO public.auctions
                (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
                 scheduled_end_at, policy_snapshot)
             VALUES ($1, $2, $3, 'Fake Auction', 'settled', $4, $4, $4, $5)",
        )
        .bind(auction_id)
        .bind(world)
        .bind(sequence)
        .bind(self.now)
        .bind(fixtures::policy_snapshot(self.now, 50_000, 30_000))
        .execute(&mut *tx)
        .await
        .expect("测试场次");
        let lot_id = Uuid::from_u128(90_000 + sequence as u128);
        batch(
            &mut tx,
            &format!(
            "INSERT INTO public.lots (id, auction_id, equipment_id, lot_number, status, start_price,
                                      category_key, category_number)
             VALUES ('{lot_id}', '{auction_id}', '{equipment}', 1, 'unsold', 50000, 'Weapon', 1)"
        ),
        )
        .await;
        tx.commit().await.expect("提交真实 lot");
        lot_id
    }

    pub(super) async fn history(&self, seller: Uuid) {
        self.history_at(seller, HISTORY, self.now).await;
    }

    pub(super) async fn history_at(
        &self,
        seller: Uuid,
        history_id: Uuid,
        blacklisted_at: Timestamp,
    ) {
        self.sql(&format!(
            "INSERT INTO public.user_blacklist_history (id,user_id,public_reason,actor,blacklisted_at)
             VALUES ('{history_id}','{seller}','Fake moderation','Fake operator','{blacklisted_at}')",
        )).await;
    }
}

pub(super) async fn batch(conn: &mut PgConnection, sql: &str) {
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(conn).await.expect("执行脱敏测试 SQL");
}

pub(super) fn seller() -> UserId {
    UserId::from_uuid(SELLER)
}

pub(super) fn window(page: u32, size: u32) -> ListingPageWindow {
    ListingPageWindow::new(Some(page), Some(size)).expect("有效分页夹具")
}

pub(super) fn public_query() -> PublicListingQuery {
    PublicListingQuery {
        window: window(1, 20),
        world: None,
        mode: None,
        category: None,
        name: None,
    }
}

pub(super) fn private_query() -> PrivateListingQuery {
    PrivateListingQuery { window: window(1, 20), world: None, mode: None, state: None }
}

pub(super) struct Listing<'a> {
    pub(super) id: Uuid,
    pub(super) seller: Uuid,
    pub(super) world: &'a str,
    pub(super) eid: &'a str,
    pub(super) mode: &'a str,
    pub(super) name: &'a str,
    pub(super) category: &'a str,
    pub(super) alias: Option<&'a str>,
    pub(super) epoch: i64,
    pub(super) created: Timestamp,
    pub(super) published: Timestamp,
    pub(super) expires: Timestamp,
}

impl<'a> Listing<'a> {
    pub(super) fn new(fixture: &Fixture, id: u128, eid: &'a str) -> Self {
        Self {
            id: Uuid::from_u128(id),
            seller: SELLER,
            world: "main",
            eid,
            mode: "direct",
            name: "Fake Blade",
            category: "Weapon",
            alias: None,
            epoch: 1,
            created: fixture.now - Duration::hours(2),
            published: fixture.now - Duration::hours(1),
            expires: fixture.now + Duration::hours(2),
        }
    }

    pub(super) fn state(&self) -> &'static str {
        if self.mode == "direct" { "displayed" } else { "awaiting_deposit" }
    }

    pub(super) async fn seed(
        &self,
        fixture: &Fixture,
        publish: bool,
        baseline: Option<Uuid>,
    ) -> (Uuid, Option<Uuid>) {
        let mut tx = fixture.pool().begin().await.expect("测试登记事务");
        let Self {
            id,
            seller,
            world,
            eid,
            mode,
            name,
            category,
            alias,
            epoch,
            created,
            published,
            expires,
        } = self;
        let deadline = *created + Duration::hours(24);
        let command_id = Uuid::new_v4();
        let fee = if *mode == "direct" { 0 } else { 500 };
        batch(&mut tx, &format!(r#"
            WITH baseline AS (
                SELECT ARRAY(SELECT id FROM public.user_blacklist_history WHERE user_id = '{seller}')
                    AS blacklist_ids
            )
            INSERT INTO public.consignment_listings
                (id,seller_user_id,world_id,external_equipment_id,mode,state,price,pricing_version,
                 fee_bps,state_version,service_epoch,blacklist_count_at_register,blacklist_baseline_ids,
                 created_at,updated_at,expires_at)
            SELECT '{id}','{seller}','{world}','{eid}','{mode}','resolving',101,1,
                   {fee},1,{epoch},cardinality(blacklist_ids),blacklist_ids,'{created}','{created}','{deadline}'
            FROM baseline;
            INSERT INTO public.consignment_commands
                (command_id,actor_kind,actor_ref,operation,request_target,request_digest,replay_key_id,
                 listing_id,result_status,result_code,result_body,created_at)
            VALUES ('{command_id}','public_user','{seller}','register','new',decode(repeat('11',32),'hex'),
                    '{REPLAY}','{id}',202,'accepted',
                    '{{"command_id":"{command_id}","operation":"register","listing_id":"{id}",
                       "listing_version":1,"state":"resolving","disposition":"registered","price":101}}',
                    '{created}');
            INSERT INTO public.consignment_events
                (listing_id,state_version,command_id,actor_kind,event_kind,to_state,price,occurred_at)
            VALUES ('{id}',1,'{command_id}','public_user','registered','resolving',101,'{created}');
            INSERT INTO public.consignment_lookup_work
                (id,listing_id,generation,state,attempt_count,fence,next_attempt_at,created_at,updated_at)
            VALUES (gen_random_uuid(),'{id}',1,'queued',0,0,'{created}','{created}','{created}');
        "#)).await;
        if let Some(lot_id) = baseline {
            batch(
                &mut tx,
                &format!(
                    "INSERT INTO public.consignment_listing_lot_baselines (listing_id,lot_id)
                 VALUES ('{id}','{lot_id}')"
                ),
            )
            .await;
        }
        let publish_id = if publish {
            let snapshot_id = Uuid::new_v4();
            let publish_id = Uuid::new_v5(id, b"apply_lookup");
            let state = self.state();
            let alias_sql = alias.map_or_else(|| "NULL".to_owned(), |value| format!("'{value}'"));
            batch(&mut tx, &format!(r#"
                INSERT INTO public.consignment_snapshots
                    (id,listing_id,generation,adapter,observed_at,digest,world_id,external_equipment_id,
                     name,category,alt_name,level_kind,level_number,tradeable,owner_name,stat_groups)
                VALUES ('{snapshot_id}','{id}',1,'hvdata-equip-v1','{published}',decode(repeat('11',32),'hex'),
                        '{world}','{eid}','{name}','{category}',{alias_sql},'assigned',100,true,
                        'FakeThirdPartyOwner','[{{"name":"Basic","stats":[{{"name":"Power","value":4.5,"base":null}}]}}]');
                UPDATE public.consignment_listings SET state = '{state}', state_version = 2,
                    current_snapshot_id = '{snapshot_id}', updated_at = '{published}', expires_at = '{expires}'
                WHERE id = '{id}';
                INSERT INTO public.consignment_commands
                    (command_id,actor_kind,actor_ref,operation,request_target,request_digest,
                     listing_id,result_status,result_code,result_body,created_at)
                VALUES ('{publish_id}','worker','{seller}','apply_lookup','{id}',decode(repeat('22',32),'hex'),
                    '{id}',200,'ok',jsonb_build_object('command_id','{publish_id}',
                        'operation','apply_lookup','listing_id','{id}','listing_version',2,'state','{state}',
                        'disposition','published','price',101,'expires_at','{expires:?}','reason_code',NULL),
                    '{published}');
                INSERT INTO public.consignment_events
                    (listing_id,state_version,command_id,actor_kind,event_kind,from_state,to_state,price,occurred_at)
                VALUES ('{id}',2,'{publish_id}','worker','published','resolving','{state}',101,'{published}');
            "#)).await;
            Some(publish_id)
        } else {
            None
        };
        tx.commit().await.expect("提交登记和发布夹具");
        (command_id, publish_id)
    }
}
