//! 凭据认证观测编排；网络期间不持有事务，结果返回前复核凭据版本。

use std::sync::Arc;

use uuid::Uuid;

use crate::ports::admin::{
    AdminCredentialCheckPort, AdminCredentialCheckResult, AdminCredentialCheckStore,
    AdminCredentialError, AdminCredentialVerifier, credential_check_worlds,
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::Clock;

/// 将授权后的只读请求绑定到数据库当前凭据，不改变 active 或 Worker 所有权。
pub struct AdminCredentialCheckService<S, V, C> {
    store: Arc<S>,
    verifier: Arc<V>,
    clock: Arc<C>,
}

impl<S, V, C> AdminCredentialCheckService<S, V, C> {
    /// 装配存储、受限外部探测和时钟。
    pub fn new(store: Arc<S>, verifier: Arc<V>, clock: Arc<C>) -> Self {
        Self { store, verifier, clock }
    }
}

impl<S, V, C> AdminCredentialCheckPort for AdminCredentialCheckService<S, V, C>
where
    S: AdminCredentialCheckStore + 'static,
    V: AdminCredentialVerifier + 'static,
    C: Clock + 'static,
{
    fn check(
        &self,
        credential_id: Uuid,
        expected_state_version: i64,
    ) -> ApplicationFuture<'_, AdminCredentialCheckResult> {
        Box::pin(async move {
            if credential_id.is_nil() || expected_state_version <= 0 {
                return Err(AdminCredentialError::InvalidRequest.into());
            }
            let record = self.store.load_for_check(credential_id).await?;
            let credential = record.credential.clone();
            if credential.id != credential_id || !credential.validate() {
                return Err(AdminCredentialError::StoredResultCorrupt.into());
            }
            if credential.state_version != expected_state_version {
                return Err(AdminCredentialError::VersionConflict {
                    current_version: credential.state_version,
                }
                .into());
            }
            let checks = self.verifier.verify(record).await?;
            let worlds = credential_check_worlds(credential.kind, credential.world);
            if checks.len() != worlds.len()
                || checks.iter().zip(worlds).any(|(check, world)| check.world != *world)
            {
                return Err(AdminCredentialError::StoredResultCorrupt.into());
            }
            // 删除、替换、启停或 Worker 失效回写发生在探测期间时，不能展示旧版本的成功。
            let current = self.store.load_for_check(credential_id).await?;
            if current.credential != credential {
                return Err(AdminCredentialError::VersionConflict {
                    current_version: current.credential.state_version,
                }
                .into());
            }
            let checked_at = self.clock.now();
            if checked_at.timestamp_nanos_opt().is_none() {
                return Err(AdminCredentialError::StoredResultCorrupt.into());
            }
            Ok(AdminCredentialCheckResult {
                credential_id,
                state_version: expected_state_version,
                checked_at,
                checks,
            })
        })
    }
}
