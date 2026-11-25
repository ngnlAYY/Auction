//! 派发轮的读写与冻结身份比对：轮行是首次 POST 前冻结身份的持久锚点。
//!
//! 保护不变量 5、6：轮、授权来源与冻结身份只追加；新轮只能紧接上一轮并引用已封闭上一轮的
//! 退件接管与消费，不允许跳号、重复或无依据重发。

use auction_domain::consignment::dispatch::{DispatchRoundKey, FrozenDispatchIdentity};
use auction_domain::consignment::{parse_settlement_world, settlement_world_code};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 已持久的派发轮。
#[derive(Debug, Clone)]
pub(super) struct StoredRound {
    pub(super) key: DispatchRoundKey,
    pub(super) round_id: Uuid,
    pub(super) delivery_id: Uuid,
    pub(super) rights_holder_identity: String,
    pub(super) bot_uid: String,
    pub(super) world_id: String,
    pub(super) subject: String,
    pub(super) template: String,
    pub(super) body_digest: Vec<u8>,
    pub(super) round_ref: String,
    pub(super) d0: Timestamp,
    pub(super) credits: i64,
    pub(super) cod: i64,
}

impl StoredRound {
    /// 冻结身份逐字段一致；时间列按数据库微秒精度比较，装备清单由投递载荷的身份摘要另行核对。
    pub(super) fn matches(&self, frozen: &FrozenDispatchIdentity) -> bool {
        self.rights_holder_identity == frozen.rights_holder_identity
            && self.bot_uid == frozen.bot_uid
            && self.world_id == settlement_world_code(frozen.world)
            && self.subject == frozen.subject
            && self.template == frozen.template
            && self.body_digest == frozen.body_digest
            && self.round_ref == frozen.round_ref
            && self.d0 == frozen.d0.trunc_subsecs(6)
            && self.credits == frozen.credits
            && self.cod == frozen.cod
    }

    /// 还原轮上冻结的派发身份；轮行不存装备清单，由调用方从投递载荷取得。世界码或正文摘要
    /// 形状异常说明数据被改坏，失败关闭。
    pub(super) fn frozen(
        &self,
        equipment_ids: Vec<String>,
    ) -> Result<FrozenDispatchIdentity, WriteError> {
        let world = parse_settlement_world(&self.world_id)
            .ok_or_else(|| WriteError::invariant("consignment_dispatch_round_world"))?;
        let body_digest = <[u8; 32]>::try_from(self.body_digest.as_slice()).map_err(|error| {
            tracing::warn!(%error, "寄售派发轮正文摘要长度异常");
            WriteError::invariant("consignment_dispatch_round_digest")
        })?;
        Ok(FrozenDispatchIdentity {
            rights_holder_identity: self.rights_holder_identity.clone(),
            bot_uid: self.bot_uid.clone(),
            world,
            subject: self.subject.clone(),
            template: self.template.clone(),
            body_digest,
            round_ref: self.round_ref.clone(),
            d0: self.d0,
            credits: self.credits,
            cod: self.cod,
            equipment_ids,
        })
    }
}

fn decode(row: &sqlx::postgres::PgRow) -> Result<StoredRound, WriteError> {
    let round: i32 = row.try_get("round")?;
    Ok(StoredRound {
        key: DispatchRoundKey {
            contract_id: row.try_get("contract_id")?,
            obligation_id: row.try_get("obligation_id")?,
            round: u32::try_from(round)?,
        },
        round_id: row.try_get("round_id")?,
        delivery_id: row.try_get("delivery_id")?,
        rights_holder_identity: row.try_get("rights_holder_identity")?,
        bot_uid: row.try_get("bot_uid")?,
        world_id: row.try_get("world_id")?,
        subject: row.try_get("subject")?,
        template: row.try_get("template")?,
        body_digest: row.try_get("body_digest")?,
        round_ref: row.try_get("round_ref")?,
        d0: row.try_get("d0")?,
        credits: row.try_get("credits")?,
        cod: row.try_get("cod")?,
    })
}

/// 按轮 ID 读取。
pub(super) async fn load_by_round_id(
    connection: &mut PgConnection,
    round_id: Uuid,
) -> Result<Option<StoredRound>, WriteError> {
    let row = sqlx::query(
        "SELECT contract_id, obligation_id, round, round_id, delivery_id,
        rights_holder_identity, bot_uid, world_id, subject, template, body_digest, round_ref, d0,
        credits, cod
     FROM public.consignment_dispatch_rounds WHERE round_id = $1",
    )
    .bind(round_id)
    .fetch_optional(connection)
    .await?;
    row.as_ref().map(decode).transpose()
}

/// 按投递读取；每个投递至多属于一个轮（`c6_round_delivery_uq`）。
pub(super) async fn load_by_delivery(
    connection: &mut PgConnection,
    delivery_id: Uuid,
) -> Result<Option<StoredRound>, WriteError> {
    let row = sqlx::query(
        "SELECT contract_id, obligation_id, round, round_id, delivery_id,
        rights_holder_identity, bot_uid, world_id, subject, template, body_digest, round_ref, d0,
        credits, cod
     FROM public.consignment_dispatch_rounds WHERE delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(connection)
    .await?;
    row.as_ref().map(decode).transpose()
}

/// 按业务键读取。
pub(super) async fn load_by_key(
    connection: &mut PgConnection,
    key: &DispatchRoundKey,
) -> Result<Option<StoredRound>, WriteError> {
    let row = sqlx::query(
        "SELECT contract_id, obligation_id, round, round_id, delivery_id,
        rights_holder_identity, bot_uid, world_id, subject, template, body_digest, round_ref, d0,
        credits, cod
     FROM public.consignment_dispatch_rounds
     WHERE contract_id = $1 AND obligation_id = $2 AND round = $3",
    )
    .bind(key.contract_id)
    .bind(key.obligation_id)
    .bind(i32::try_from(key.round)?)
    .fetch_optional(connection)
    .await?;
    row.as_ref().map(decode).transpose()
}

/// 同义务已有的最大轮次号。
pub(super) async fn latest_round(
    connection: &mut PgConnection,
    contract_id: Uuid,
    obligation_id: Uuid,
) -> Result<Option<u32>, WriteError> {
    let latest: Option<i32> = sqlx::query_scalar(
        "SELECT max(round) FROM public.consignment_dispatch_rounds
         WHERE contract_id = $1 AND obligation_id = $2",
    )
    .bind(contract_id)
    .bind(obligation_id)
    .fetch_one(connection)
    .await?;
    latest.map(u32::try_from).transpose().map_err(WriteError::from)
}

/// 新轮的授权来源：首轮没有，后续轮引用封闭上一轮的接管与消费。
#[derive(Debug, Clone, Copy)]
pub(super) struct Authorization {
    pub(super) prior_round: Option<u32>,
    pub(super) intake_id: Option<Uuid>,
    pub(super) consumption_id: Option<Uuid>,
}

impl Authorization {
    /// 首轮：没有上一轮与授权来源。
    pub(super) const fn first() -> Self {
        Self { prior_round: None, intake_id: None, consumption_id: None }
    }
}

/// 插入轮行；`c6_round_authorization_ck` 与外键在数据库侧再核一遍授权来源。
pub(super) async fn insert_round(
    connection: &mut PgConnection,
    key: &DispatchRoundKey,
    round_id: Uuid,
    delivery_id: Uuid,
    frozen: &FrozenDispatchIdentity,
    authorization: Authorization,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.consignment_dispatch_rounds
            (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
             bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
             prior_round, authorized_by_intake, authorized_by_consumption)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
    )
    .bind(key.contract_id)
    .bind(key.obligation_id)
    .bind(i32::try_from(key.round)?)
    .bind(round_id)
    .bind(delivery_id)
    .bind(&frozen.rights_holder_identity)
    .bind(&frozen.bot_uid)
    .bind(settlement_world_code(frozen.world))
    .bind(&frozen.subject)
    .bind(&frozen.template)
    .bind(frozen.body_digest.as_slice())
    .bind(&frozen.round_ref)
    .bind(frozen.d0)
    .bind(frozen.credits)
    .bind(frozen.cod)
    .bind(authorization.prior_round.map(i32::try_from).transpose()?)
    .bind(authorization.intake_id)
    .bind(authorization.consumption_id)
    .execute(connection)
    .await?;
    Ok(())
}
