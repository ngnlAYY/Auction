//! 登记逻辑比较钥初始化、MAC 证明及登记信封生命周期。

use auction_application::consignment::{
    ConsignmentError, RegistrationInput, RegistrationReplayCrypto, WrappedRegistrationReplayKey,
};
use auction_domain::{EquipmentId, EquipmentKeyEnvelope};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::errors::WriteError;
use super::ledger::RegistrationProof;
use super::session::{LockKeys, with_command_transaction};
use super::target::now;
use crate::database::DatabaseStore;
use crate::database::consignment::lookup::control::lock_control;

impl DatabaseStore {
    /// Public 以已同步的启用配置装配登记时，确认或初始化当前逻辑比较钥。
    /// 先核持久准入，关闭时不落行；对同一个候选 UUID 重试仍只采用当前逻辑钥。
    /// 不变量 1、2、7：与登记共享唯一索引和锁序，明文比较钥只在本地密码端口内使用。
    pub async fn initialize_listing_replay_key(
        &self,
        candidate_id: Uuid,
        crypto: &dyn RegistrationReplayCrypto,
    ) -> Result<(), ConsignmentError> {
        let keys = LockKeys { command_id: candidate_id, sellers: &[], equipment: &[] };
        with_command_transaction(
            self,
            keys,
            &(candidate_id, crypto),
            |&(candidate_id, crypto), tx| {
                Box::pin(async move {
                    let conn = tx.connection();
                    if !lock_control(conn).await?.admitting {
                        return Err(ConsignmentError::Unavailable.into());
                    }
                    let as_of = now(conn).await?;
                    let (replay_key_id, wrapped) =
                        current_replay_key(conn, crypto, candidate_id, as_of).await?;
                    crypto
                        .check_wrapped(replay_key_id, &wrapped)
                        .map_err(ConsignmentError::from)?;
                    Ok(())
                })
            },
        )
        .await
        .map_err(WriteError::into_application)
    }
}

/// 必须先锁定解析准入单例。并发首次登记只建立一个当前逻辑钥，不变量 1、2、7。
pub(super) async fn registration_proof(
    conn: &mut PgConnection,
    crypto: &dyn RegistrationReplayCrypto,
    candidate_id: Uuid,
    input: &RegistrationInput<'_>,
    now: Timestamp,
) -> Result<RegistrationProof, WriteError> {
    let (replay_key_id, wrapped) = current_replay_key(conn, crypto, candidate_id, now).await?;
    let mac =
        crypto.registration_mac(replay_key_id, &wrapped, input).map_err(ConsignmentError::from)?;
    Ok(RegistrationProof { replay_key_id, mac })
}

async fn current_replay_key(
    conn: &mut PgConnection,
    crypto: &dyn RegistrationReplayCrypto,
    candidate_id: Uuid,
    now: Timestamp,
) -> Result<(Uuid, WrappedRegistrationReplayKey), WriteError> {
    let current: Option<(Uuid, Vec<u8>, Vec<u8>, String)> = sqlx::query_as(
        "SELECT id, wrapped_key, nonce, encryption_key_id FROM public.consignment_replay_keys
         WHERE write_current",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let current = if let Some((replay_key_id, ciphertext, nonce, key_id)) = current {
        let Ok(nonce) = nonce.try_into() else {
            return Err(ConsignmentError::Unavailable.into());
        };
        (
            replay_key_id,
            WrappedRegistrationReplayKey::new(ciphertext, nonce, key_id)
                .map_err(ConsignmentError::from)?,
        )
    } else {
        let wrapped = crypto.generate_wrapped(candidate_id).map_err(ConsignmentError::from)?;
        let inserted = sqlx::query(
            "INSERT INTO public.consignment_replay_keys
                (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
             VALUES ($1, true, $2, $3, $4, $5, $5)
             ON CONFLICT (write_current) WHERE write_current DO NOTHING",
        )
        .bind(candidate_id)
        .bind(wrapped.wrapped_key())
        .bind(wrapped.nonce().as_slice())
        .bind(wrapped.encryption_key_id())
        .bind(now)
        .execute(&mut *conn)
        .await?;
        // Serializable 下，快照外的胜出行由 PostgreSQL 返回 40001。若没有插入且没有
        // 可读当前项，则失败关闭，不能以本地候选项开始写命令。
        if inserted.rows_affected() != 1 {
            return Err(ConsignmentError::Unavailable.into());
        }
        (candidate_id, wrapped)
    };
    Ok(current)
}

/// 只绑定已封装的字段。登记的 AAD 与游戏 EID 和挂单 UUID 分开。不变量 3、7。
pub(super) async fn insert_secret(
    conn: &mut PgConnection,
    listing_id: Uuid,
    aad_equipment_id: EquipmentId,
    envelope: &EquipmentKeyEnvelope,
    now: Timestamp,
) -> Result<(), WriteError> {
    // 既有装备信封允许历史指纹格式；寄售表只接受当前生成的 64 位小写十六进制。
    let fingerprint = envelope.fingerprint();
    if fingerprint.len() != 64
        || !fingerprint.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ConsignmentError::Unavailable.into());
    }
    sqlx::query(
        "INSERT INTO public.consignment_registration_secrets
            (listing_id, aad_equipment_id, ciphertext, nonce, encryption_key_id, fingerprint,
             created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
    )
    .bind(listing_id)
    .bind(aad_equipment_id.as_uuid())
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(fingerprint)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// 解析发布、拒绝或关闭时不可逆地擦除信封，保留原 AAD 与退休时刻。不变量 3、7。
pub(super) async fn retire_secret(
    conn: &mut PgConnection,
    listing_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE public.consignment_registration_secrets
         SET ciphertext = NULL, nonce = NULL, encryption_key_id = NULL, fingerprint = NULL,
             updated_at = GREATEST(updated_at, $2), retired_at = GREATEST(updated_at, $2)
         WHERE listing_id = $1 AND retired_at IS NULL",
    )
    .bind(listing_id)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}
