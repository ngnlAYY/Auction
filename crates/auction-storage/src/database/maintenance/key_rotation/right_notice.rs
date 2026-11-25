//! 权利通知 outbox 的挑战码密文：凭据域轮换的扫描投影、载荷拆分与挑战 AAD。
//!
//! `sealed_payload` 的布局是 `密文 ‖ 64 字节小写十六进制明文指纹`（见 rights/outbox.rs）；轮换时按这个
//! 布局拆开交给调用方解封，写回时再按同一布局拼回。AAD 取自挑战行冻结的权利、身份摘要与权利版本，
//! 与发码时的封装输入一致，所以重封不改变 AAD。已清空密文的行（无秘密的通知种类）不在扫描范围内。

use auction_application::consignment::ChallengeAad;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::KeyRotationError;
use super::read_failed;

/// 指纹十六进制文本的字节数，位于 `sealed_payload` 末尾。
const FINGERPRINT_LEN: usize = 64;

/// 与通用信封扫描同形的投影；派生表沿用 outbox 表名，使通用的游标与钥过滤条件不需要限定。
pub(super) const SELECT: &str = "SELECT work_id AS entity_id, NULL::uuid AS equipment_aad,
        false AS retired, NULL::text AS credential_kind, NULL::text AS scope,
        NULL::bigint AS generation, NULL::uuid AS seller_user_id, NULL::text AS purpose,
        NULL::text AS aad_owner, ciphertext, nonce, encryption_key_id, ''::text AS fingerprint,
        challenge_id, right_id, identity_digest, right_version
    FROM (
        SELECT outbox.work_id, outbox.sealed_payload AS ciphertext, outbox.nonce,
               outbox.encryption_key_id, outbox.challenge_id, outbox.right_id,
               challenge.identity_digest, challenge.right_version
        FROM public.consignment_right_notice_outbox AS outbox
        LEFT JOIN public.consignment_right_challenges AS challenge
            ON challenge.challenge_id = outbox.challenge_id
        WHERE outbox.sealed_payload IS NOT NULL
    ) AS consignment_right_notice_outbox";

/// 写回的 SET 片段与在用条件：按原布局拼接新密文与新指纹（`$1` 密文、`$7` 指纹）。outbox 没有 `updated_at` 列，
/// 000137 的受控守卫分支只放行密文三列整体换新。
pub(super) const REPLACE: (&str, &str) =
    ("sealed_payload = $1 || convert_to($7, 'UTF8')", "AND sealed_payload IS NOT NULL");

/// 写回时附带的 `updated_at` 维护片段；outbox 没有该列。
pub(super) const fn touched(notice: bool) -> &'static str {
    if notice { "" } else { ", updated_at = GREATEST(transaction_timestamp(), updated_at)" }
}

/// 读取信封的密文与指纹：outbox 行从载荷拆出，其余来源各有独立列。
pub(super) fn envelope_bytes(
    row: &PgRow,
    notice: bool,
) -> Result<(Vec<u8>, String), KeyRotationError> {
    if notice {
        return split_payload(row.try_get("ciphertext").map_err(read_failed)?);
    }
    Ok((
        row.try_get("ciphertext").map_err(read_failed)?,
        row.try_get("fingerprint").map_err(read_failed)?,
    ))
}

/// 拆开 `密文 ‖ 指纹`；形状不合法时失败关闭，不把残缺密文交给调用方。
fn split_payload(mut payload: Vec<u8>) -> Result<(Vec<u8>, String), KeyRotationError> {
    let split = payload.len().checked_sub(FINGERPRINT_LEN).filter(|split| *split > 0);
    let Some(split) = split else {
        payload.fill(0);
        return Err(KeyRotationError::Read);
    };
    let fingerprint = String::from_utf8(payload.split_off(split));
    let Ok(fingerprint) = fingerprint else {
        payload.fill(0);
        return Err(KeyRotationError::Read);
    };
    Ok((payload, fingerprint))
}

/// 挑战行冻结的 AAD；任一列缺失或形状不合法时为 `None`，由调用方失败关闭。
pub(super) fn challenge_aad(row: &PgRow) -> Result<Option<ChallengeAad>, KeyRotationError> {
    let challenge_id: Option<Uuid> = row.try_get("challenge_id").map_err(read_failed)?;
    let right_id: Option<Uuid> = row.try_get("right_id").map_err(read_failed)?;
    let digest: Option<Vec<u8>> = row.try_get("identity_digest").map_err(read_failed)?;
    let version: Option<i64> = row.try_get("right_version").map_err(read_failed)?;
    let (Some(challenge_id), Some(right_id), Some(digest), Some(version)) =
        (challenge_id, right_id, digest, version)
    else {
        return Ok(None);
    };
    let (Ok(identity_digest), Ok(right_version)) =
        (<[u8; 32]>::try_from(digest), u64::try_from(version))
    else {
        return Ok(None);
    };
    Ok(Some(ChallengeAad {
        challenge_id,
        right_id: Some(right_id),
        identity_digest,
        right_version,
    }))
}
