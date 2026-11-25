//! 单权利会话：持久令牌摘要、动作范围、浏览器绑定与原截止；同命令恢复只轮换令牌。
//!
//! 不变量 1、2、7：令牌只存 SHA-256 摘要，`generation` 最大者是当前令牌，旧令牌即刻失效；
//! 会话截止在授予时固定，轮换只追加令牌行、绝不延寿；浏览器绑定不符一律按未认证处理。

use auction_application::consignment::{BrowserBinding, RightsStoreError};
use auction_domain::consignment::rights::{
    RightAction, RightSessionToken, SESSION_TTL, SessionDigest, rotated_session_expiry,
    session_digest,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{TxError, random_bytes};

/// 令牌对应的、仍有效的会话。
pub(super) struct SessionRow {
    pub(super) right_id: Uuid,
    pub(super) scope: Vec<RightAction>,
}

/// 新授予会话的令牌与截止。
pub(super) struct IssuedSession {
    pub(super) token: RightSessionToken,
    pub(super) expires_at: Timestamp,
}

fn corrupt() -> TxError {
    tracing::warn!("受限权利会话行形状损坏");
    TxError::Store(RightsStoreError::Unavailable)
}

/// 按令牌摘要认证：必须是该会话的当前令牌、浏览器绑定相符、未过原截止；否则 `None`。
pub(super) async fn authenticate(
    connection: &mut PgConnection,
    token: SessionDigest,
    browser: BrowserBinding,
    now: Timestamp,
) -> Result<Option<SessionRow>, TxError> {
    let row = sqlx::query(
        "SELECT s.right_id, s.action_scope, s.browser_binding_digest, s.expires_at
         FROM public.consignment_right_session_tokens AS t
         JOIN public.consignment_right_sessions AS s ON s.session_id = t.session_id
         WHERE t.token_digest = $1
           AND NOT EXISTS (
               SELECT 1 FROM public.consignment_right_session_tokens AS newer
               WHERE newer.session_id = t.session_id AND newer.generation > t.generation)",
    )
    .bind(token.as_bytes().as_slice())
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let bound: &[u8] = row.try_get("browser_binding_digest")?;
    let expires_at: Timestamp = row.try_get("expires_at")?;
    if !bool::from(bound.ct_eq(browser.0.as_slice())) || now >= expires_at {
        return Ok(None);
    }
    let scope = row
        .try_get::<Vec<String>, _>("action_scope")?
        .iter()
        .map(|text| RightAction::parse(text))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(corrupt)?;
    Ok(Some(SessionRow { right_id: row.try_get("right_id")?, scope }))
}

/// 授予会话：一次挑战至多一个会话（`c7_session_challenge_uq`），首个令牌代数为 1。
pub(super) async fn grant(
    connection: &mut PgConnection,
    challenge_id: Uuid,
    right_id: Uuid,
    scope: &[RightAction],
    browser: BrowserBinding,
    now: Timestamp,
) -> Result<IssuedSession, TxError> {
    let expires_at = now.checked_add_signed(SESSION_TTL).ok_or_else(corrupt)?;
    let session_id = Uuid::new_v4();
    let scope: Vec<&str> = scope.iter().map(|action| action.as_str()).collect();
    sqlx::query(
        "INSERT INTO public.consignment_right_sessions
            (session_id, challenge_id, right_id, action_scope, browser_binding_digest,
             issued_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(session_id)
    .bind(challenge_id)
    .bind(right_id)
    .bind(&scope)
    .bind(browser.0.as_slice())
    .bind(now)
    .bind(expires_at)
    .execute(&mut *connection)
    .await?;
    let token = insert_token(connection, session_id, now).await?;
    Ok(IssuedSession { token, expires_at })
}

/// 同命令恢复：沿用原截止，只换令牌；旧令牌即刻失效。原浏览器不符或已过原截止时失败。
pub(super) async fn rotate(
    connection: &mut PgConnection,
    challenge_id: Uuid,
    browser: BrowserBinding,
    now: Timestamp,
) -> Result<IssuedSession, TxError> {
    let row = sqlx::query(
        "SELECT session_id, browser_binding_digest, expires_at
         FROM public.consignment_right_sessions WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(RightsStoreError::RightExpired)?;
    let bound: &[u8] = row.try_get("browser_binding_digest")?;
    if !bool::from(bound.ct_eq(browser.0.as_slice())) {
        return Err(RightsStoreError::ChallengeInvalid.into());
    }
    let expires_at =
        rotated_session_expiry(row.try_get("expires_at")?, now).map_err(RightsStoreError::from)?;
    let token = insert_token(connection, row.try_get("session_id")?, now).await?;
    Ok(IssuedSession { token, expires_at })
}

async fn insert_token(
    connection: &mut PgConnection,
    session_id: Uuid,
    now: Timestamp,
) -> Result<RightSessionToken, TxError> {
    let token = RightSessionToken::from_entropy(random_bytes::<32>());
    sqlx::query(
        "INSERT INTO public.consignment_right_session_tokens
            (session_id, generation, token_digest, issued_at)
         SELECT $1, COALESCE(max(generation), 0) + 1, $2, $3
         FROM public.consignment_right_session_tokens WHERE session_id = $1",
    )
    .bind(session_id)
    .bind(session_digest(&token).as_bytes().as_slice())
    .bind(now)
    .execute(connection)
    .await?;
    Ok(token)
}
