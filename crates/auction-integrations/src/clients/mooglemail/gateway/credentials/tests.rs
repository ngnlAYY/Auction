//! 凭据会话的 owner 探测回归：外层预算取消只放弃等待，不撤销邮件运行时所有权。

use std::sync::Arc;
use std::time::{Duration, Instant};

use auction_application::admin::{AdminCredentialRuntimeOwnership, AdminCredentialRuntimeWriter};
use auction_application::core::ApplicationFuture;
use auction_application::settlement::InternalEvidenceError;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{MoogleMailCredentialSet, MoogleMailRuntimeCredential};
use crate::clients::mooglemail::{MoogleMailCookie, MoogleMailOperation, MoogleMailWorld};
use crate::clients::owner_probe::fixture::GatedOwnership;

const COOKIE: &str = "ipb_member_id=fixture; ipb_pass_hash=fixture";

/// 回写失效只在远端确认登录失效时发生；本文件的用例不应触发。
struct NoInvalidation;

impl AdminCredentialRuntimeWriter for NoInvalidation {
    fn mark_invalid(&self, _credential_id: Uuid, _now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

fn cookie() -> MoogleMailCookie {
    MoogleMailCookie::new(COOKIE).expect("fixture cookie")
}

/// 带持久身份与 owner 的数据库来源集合，满足内部证据的绑定前提。
fn runtime_set(owner: Arc<dyn AdminCredentialRuntimeOwnership>) -> MoogleMailCredentialSet {
    MoogleMailCredentialSet::from_runtime_credentials(
        vec![MoogleMailRuntimeCredential {
            credential_id: Uuid::new_v4(),
            state_version: 1,
            world: None,
            player_name: None,
            cookie: cookie(),
        }],
        Arc::new(NoInvalidation),
        Some(owner),
    )
    .expect("runtime credential set")
}

#[tokio::test]
async fn read_session_owner_probe_survives_outer_cancellation() {
    let gated = Arc::new(GatedOwnership::default());
    let owner: Arc<dyn AdminCredentialRuntimeOwnership> = gated.clone();
    let set = MoogleMailCredentialSet::from_owned_cookie(cookie(), Arc::clone(&owner))
        .expect("owned credential set");

    // begin_read 在 check_current 中等待 owner 探测；外层 1ms 预算先到期。
    let outer = tokio::time::timeout(
        Duration::from_millis(1),
        set.begin_read(MoogleMailWorld::Main, MoogleMailOperation::Inbox),
    )
    .await;
    assert!(outer.is_err(), "外层预算应先到期");
    assert!(owner.is_valid(), "外层取消不得撤销 owner");

    gated.release_and_wait().await;
    assert!(gated.completed());
    assert!(owner.is_valid());
    assert!(set.available(MoogleMailWorld::Main), "凭据集合仍可服务后续读取");
}

#[tokio::test]
async fn internal_probe_deadline_expires_without_revoking_the_owner() {
    let gated = Arc::new(GatedOwnership::default());
    let owner: Arc<dyn AdminCredentialRuntimeOwnership> = gated.clone();
    let set = runtime_set(Arc::clone(&owner));
    let session = set
        .begin_exclusive(MoogleMailWorld::Main, MoogleMailOperation::Compose)
        .await
        .expect("exclusive session");

    let deadline = Instant::now() + Duration::from_millis(20);
    let error = session.check_internal_current(deadline).await.expect_err("探测挂起时预算先到期");
    assert!(matches!(error, InternalEvidenceError::Expired));
    assert!(owner.is_valid(), "证据预算到期不得撤销 owner");

    gated.release_and_wait().await;
    assert!(owner.is_valid());
    assert!(set.available(MoogleMailWorld::Main));
}

#[tokio::test]
async fn internal_probe_failure_is_reported_as_revoked() {
    let owner: Arc<dyn AdminCredentialRuntimeOwnership> = Arc::new(GatedOwnership::failing());
    let set = runtime_set(owner);
    let session = set
        .begin_exclusive(MoogleMailWorld::Main, MoogleMailOperation::Compose)
        .await
        .expect("exclusive session");

    let error = session
        .check_internal_current(Instant::now() + Duration::from_secs(5))
        .await
        .expect_err("owner 失效必须失败关闭");
    assert!(matches!(error, InternalEvidenceError::Revoked));
}
