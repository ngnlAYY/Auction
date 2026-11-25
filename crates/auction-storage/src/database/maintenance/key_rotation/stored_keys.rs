//! 各数据密钥域实际出现的信封钥 ID 与数量。

use auction_domain::DataKeyDomain;
use sqlx::{PgPool, Row};

use super::KeyRotationError;
use super::read_failed;

/// 列出该域实际出现的 key ID 与信封数量；用于核对旧密钥是否仍被引用。
pub async fn stored_key_ids(
    pool: &PgPool,
    domain: DataKeyDomain,
) -> Result<Vec<(String, i64)>, KeyRotationError> {
    let query = match domain {
        DataKeyDomain::Equipment => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM (
                 SELECT encryption_key_id FROM public.equipment_secrets
                 UNION ALL
                 SELECT encryption_key_id FROM public.consignment_registration_secrets
                 WHERE retired_at IS NULL
                 UNION ALL
                 SELECT encryption_key_id FROM public.consignment_replay_keys
             ) AS envelopes
             GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::UserLogin => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM (
                 SELECT encryption_key_id
                 FROM public.user_login_key_envelopes
                 UNION ALL
                 SELECT response_encryption_key_id AS encryption_key_id
                 FROM public.user_admin_commands
                 WHERE response_encryption_key_id IS NOT NULL
             ) AS envelopes
             GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::Credential => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM (
                 SELECT encryption_key_id FROM public.credential_keys
                 UNION ALL
                 SELECT encryption_key_id FROM public.consignment_reservation_secrets
                 WHERE retired_at IS NULL
                 UNION ALL
                 SELECT encryption_key_id FROM public.mail_evidence_envelopes
                 WHERE retired_at IS NULL
                 UNION ALL
                 SELECT encryption_key_id FROM public.consignment_right_notice_outbox
                 WHERE sealed_payload IS NOT NULL
             ) AS envelopes
             GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::MailDiagnostics => return Err(KeyRotationError::UnsupportedDomain),
    };
    let rows = sqlx::query(query).fetch_all(pool).await.map_err(read_failed)?;
    rows.into_iter()
        .map(|row| {
            let key_id: String = row.try_get("encryption_key_id").map_err(read_failed)?;
            let count: i64 = row.try_get("envelopes").map_err(read_failed)?;
            Ok((key_id, count))
        })
        .collect()
}
