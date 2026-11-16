//! 凭据认证观测编排；网络期间不持有事务，结果返回前复核凭据版本。

use std::sync::Arc;

use uuid::Uuid;

use crate::ports::admin::{
    AdminCredentialCheckPort, AdminCredentialCheckResult, AdminCredentialCheckStore,
    AdminCredentialError, AdminCredentialVerifier, credential_check_worlds,
};
use crate::ports::core::ApplicationFuture;
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, TimeZone, Utc};
    use uuid::Uuid;

    use super::AdminCredentialCheckService;
    use crate::errors::ApplicationError;
    use crate::ports::admin::{
        AdminCredential, AdminCredentialCheckObservation, AdminCredentialCheckPort,
        AdminCredentialCheckRecord, AdminCredentialCheckStatus, AdminCredentialCheckStore,
        AdminCredentialEnvelope, AdminCredentialError, AdminCredentialKind, AdminCredentialStatus,
        AdminCredentialVerifier, AdminCredentialWorld,
    };
    use crate::ports::core::{ApplicationFuture, Clock};

    const CREDENTIAL: Uuid = Uuid::from_u128(0x77);

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 8, 0, 0).single().expect("fixed time")
    }

    fn credential(state_version: i64, status: AdminCredentialStatus) -> AdminCredential {
        AdminCredential {
            id: CREDENTIAL,
            kind: AdminCredentialKind::MailCookie,
            world: AdminCredentialWorld::Main,
            label: "backup".to_owned(),
            player_name: None,
            status,
            state_version,
            created_at: at(),
            rotated_at: None,
            updated_at: at(),
        }
    }

    /// 每次读取依次返回脚本中的凭据投影；探测前后各读一次。
    struct ScriptedStore(Mutex<Vec<AdminCredential>>);

    impl AdminCredentialCheckStore for ScriptedStore {
        fn load_for_check(
            &self,
            _credential_id: Uuid,
        ) -> ApplicationFuture<'_, AdminCredentialCheckRecord> {
            let credential = self.0.lock().unwrap().remove(0);
            Box::pin(async move {
                Ok(AdminCredentialCheckRecord {
                    credential,
                    envelope: AdminCredentialEnvelope::new(
                        vec![1; 32],
                        vec![2; 12],
                        "fixture-key".to_owned(),
                        "fixture-fingerprint".to_owned(),
                    )?,
                })
            })
        }
    }

    /// 计数并返回固定观测的探测器。
    struct CountingVerifier {
        calls: AtomicUsize,
        checks: Vec<AdminCredentialCheckObservation>,
    }

    impl AdminCredentialVerifier for CountingVerifier {
        fn verify(
            &self,
            _record: AdminCredentialCheckRecord,
        ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let checks = self.checks.clone();
            Box::pin(async move { Ok(checks) })
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            at()
        }
    }

    fn main_valid() -> Vec<AdminCredentialCheckObservation> {
        vec![AdminCredentialCheckObservation {
            world: AdminCredentialWorld::Main,
            status: AdminCredentialCheckStatus::Valid,
        }]
    }

    fn service(
        loads: Vec<AdminCredential>,
        checks: Vec<AdminCredentialCheckObservation>,
    ) -> (
        AdminCredentialCheckService<ScriptedStore, CountingVerifier, FixedClock>,
        Arc<CountingVerifier>,
    ) {
        let verifier = Arc::new(CountingVerifier { calls: AtomicUsize::new(0), checks });
        let service = AdminCredentialCheckService::new(
            Arc::new(ScriptedStore(Mutex::new(loads))),
            Arc::clone(&verifier),
            Arc::new(FixedClock),
        );
        (service, verifier)
    }

    /// 只轮询立即就绪的假端口，不引入额外异步运行时依赖。
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    fn version_conflict(result: Result<impl std::fmt::Debug, ApplicationError>) -> Option<i64> {
        match result {
            Err(ApplicationError::AdminCredential(AdminCredentialError::VersionConflict {
                current_version,
            })) => Some(current_version),
            _ => None,
        }
    }

    #[test]
    fn disabled_credential_at_the_expected_version_is_checked() {
        let disabled = credential(4, AdminCredentialStatus::Disabled);
        let (service, verifier) = service(vec![disabled.clone(), disabled], main_valid());
        let result = block_on(service.check(CREDENTIAL, 4)).expect("检查完成");
        assert_eq!(result.state_version, 4);
        assert_eq!(result.checked_at, at());
        assert_eq!(result.checks, main_valid());
        assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_expected_version_is_rejected_before_probing() {
        let (service, verifier) =
            service(vec![credential(5, AdminCredentialStatus::Active)], main_valid());
        assert_eq!(version_conflict(block_on(service.check(CREDENTIAL, 4))), Some(5));
        assert_eq!(verifier.calls.load(Ordering::SeqCst), 0, "版本不符时不得发起外部探测");
    }

    #[test]
    fn credential_changed_during_probe_never_reports_the_old_result() {
        let (service, verifier) = service(
            vec![
                credential(4, AdminCredentialStatus::Active),
                credential(5, AdminCredentialStatus::Invalid),
            ],
            main_valid(),
        );
        assert_eq!(version_conflict(block_on(service.check(CREDENTIAL, 4))), Some(5));
        assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn observations_must_cover_exactly_the_credential_worlds() {
        let active = credential(4, AdminCredentialStatus::Active);
        let isekai = vec![AdminCredentialCheckObservation {
            world: AdminCredentialWorld::Isekai,
            status: AdminCredentialCheckStatus::Valid,
        }];
        let (service, _) = service(vec![active.clone(), active], isekai);
        assert!(matches!(
            block_on(service.check(CREDENTIAL, 4)),
            Err(ApplicationError::AdminCredential(AdminCredentialError::StoredResultCorrupt))
        ));
    }
}
