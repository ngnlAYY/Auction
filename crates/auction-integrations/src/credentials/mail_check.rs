//! Worker 侧的邮件凭据在线检查：Admin 经 Worker control 转来的检查在本进程按 ID 打开数据库信封，
//! 经与邮件循环共用的 gateway 以候选身份读取余额页。被检查的可以是 disabled/invalid 凭据，绝不以
//! 当前运行 Cookie 代替；不借用运行凭据会话、不改变凭据状态，成功也不结束当前账号的忙碌期。

use std::{sync::Arc, time::Duration};

use auction_application::admin::{
    AdminCredentialCheckObservation, AdminCredentialCheckRecord, AdminCredentialCheckStatus,
    AdminCredentialError, AdminCredentialKind, AdminCredentialOpener, AdminCredentialVerifier,
    AdminCredentialWorld, credential_check_worlds,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::runtime_switches::RuntimeSwitches;
use tokio::sync::Semaphore;

use super::DecryptedCookie;
use crate::mooglemail::{
    MoogleMailCookie, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailWorld,
};
use crate::safe_diagnostics;

/// 单个世界的探测预算，含请求门与在途锁排队；shared 凭据最多两个世界，合计仍在转发预算内。
const WORLD_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// Worker 进程内执行的邮件 Cookie 探测器；只接受数据库信封，只检查邮件 Cookie。
pub struct WorkerMailCredentialVerifier {
    opener: Arc<dyn AdminCredentialOpener>,
    gateway: Arc<MoogleMailGateway>,
    switches: RuntimeSwitches,
    slots: Semaphore,
}

impl WorkerMailCredentialVerifier {
    /// `gateway` 必须是本进程邮件循环使用的实例，探测才与 Worker 自己的读写共用请求门、在途锁、
    /// 账号忙熔断与运行 owner；`switches` 提供主动暂停外部访问的开关。
    pub fn new(
        opener: Arc<dyn AdminCredentialOpener>,
        gateway: Arc<MoogleMailGateway>,
        switches: RuntimeSwitches,
    ) -> Self {
        Self { opener, gateway, switches, slots: Semaphore::new(1) }
    }

    async fn check_world(
        &self,
        world: AdminCredentialWorld,
        secret: &[u8],
    ) -> AdminCredentialCheckStatus {
        use AdminCredentialCheckStatus::{Inconclusive, Invalid, Valid};
        let world = match world {
            AdminCredentialWorld::Main => MoogleMailWorld::Main,
            AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
            AdminCredentialWorld::Shared => return Inconclusive,
        };
        let Ok(secret) = std::str::from_utf8(secret) else {
            tracing::debug!(
                world = world.as_str(),
                reason = "not_utf8",
                "邮件凭据检查无法得出结论"
            );
            return Inconclusive;
        };
        let Ok(cookie) = MoogleMailCookie::new(secret) else {
            tracing::debug!(
                world = world.as_str(),
                reason = "invalid_cookie",
                "邮件凭据检查无法得出结论"
            );
            return Inconclusive;
        };
        // Item Shop 只读余额页不触发邮件打开、取附件、compose、send 或战斗操作。Valid 只表示当次
        // Cookie 可用；余额不是自身名称证据，账号身份只由内部组合证据端口联合证明。
        match self.gateway.probe_candidate_cookie(world, &cookie).await {
            Ok(()) => Valid,
            Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => Invalid,
            Err(diagnostic_cause) => {
                safe_diagnostics::warn(
                    "credential_check",
                    "verify_game_session",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                Inconclusive
            }
        }
    }
}

impl AdminCredentialVerifier for WorkerMailCredentialVerifier {
    fn verify(
        &self,
        record: AdminCredentialCheckRecord,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialCheckObservation>> {
        Box::pin(async move {
            let credential = &record.credential;
            if credential.kind != AdminCredentialKind::MailCookie {
                return Err(AdminCredentialError::InvalidRequest.into());
            }
            // 主动暂停外部访问期间不发起任何请求；Admin 只看到“暂不可用”。
            if self.switches.external_access_paused() {
                return Err(AdminCredentialError::CheckUnavailable.into());
            }
            // 立即拒绝重叠检查：同一账号的物理请求本就串行，排队只会挤占 Worker 自己的读写。
            let _permit = self.slots.try_acquire().ok().ok_or(AdminCredentialError::CheckBusy)?;
            let secret = DecryptedCookie(self.opener.open(
                credential.kind,
                credential.id,
                &credential.world.scope(credential.kind),
                &record.envelope,
            )?);
            let mut checks = Vec::new();
            for world in credential_check_worlds(credential.kind, credential.world) {
                // 预算含排队；超时只说明这次没有结论，不重试。
                let status =
                    tokio::time::timeout(WORLD_CHECK_BUDGET, self.check_world(*world, &secret.0))
                        .await
                        .unwrap_or(AdminCredentialCheckStatus::Inconclusive);
                checks.push(AdminCredentialCheckObservation { world: *world, status });
            }
            Ok(checks)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use auction_application::admin::{
        AdminCredential, AdminCredentialCheckObservation, AdminCredentialCheckRecord,
        AdminCredentialCheckStatus, AdminCredentialError, AdminCredentialKind,
        AdminCredentialSealer, AdminCredentialStatus, AdminCredentialVerifier,
        AdminCredentialWorld,
    };
    use auction_application::core::ApplicationError;
    use auction_application::runtime_switches::{
        RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchRead,
        RuntimeSwitches,
    };
    use auction_domain::DataKeyDomain;
    use auction_test_support::mooglemail::{Fault, MailDouble};
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use chrono::{TimeDelta, Utc};
    use uuid::Uuid;

    use super::WorkerMailCredentialVerifier;
    use crate::credentials::CredentialSealer;
    use crate::keyring::Keyring;
    use crate::mooglemail::{
        MoogleMailClient, MoogleMailClientConfig, MoogleMailCookie, MoogleMailCredentialSet,
        MoogleMailErrorCategory, MoogleMailGateway, MoogleMailGatewayConfig, MoogleMailWorld,
    };

    /// 当前运行账号的 Cookie；候选凭据检查绝不能改用它。
    const ACTIVE_COOKIE: &str = "ipb_member_id=active; ipb_pass_hash=active";
    /// Admin 指定检查的候选凭据明文。
    const CANDIDATE_COOKIE: &str = "ipb_member_id=candidate; ipb_pass_hash=candidate";
    const CREDENTIAL: Uuid = Uuid::from_u128(0x5eed);

    struct Fixture {
        server: MailDouble,
        /// Worker 自己的读写所用客户端；与 gateway 共享请求门、在途锁与账号忙状态。
        client: Arc<MoogleMailClient>,
        sealer: Arc<CredentialSealer>,
        switches: RuntimeSwitches,
        verifier: WorkerMailCredentialVerifier,
    }

    async fn fixture() -> Fixture {
        let server = MailDouble::start().await.expect("loopback server");
        let client = Arc::new(
            MoogleMailClient::with_config(MoogleMailClientConfig {
                endpoint: server.endpoint(),
                maximum_response_bytes: 8 * 1024 * 1024,
                minimum_request_interval: Duration::from_millis(1),
                request_timeout: Duration::from_secs(5),
            })
            .expect("loopback client"),
        );
        let gateway = MoogleMailGateway::new(
            Arc::clone(&client),
            MoogleMailCredentialSet::new(ACTIVE_COOKIE).expect("active cookie"),
            MoogleMailGatewayConfig::default(),
        )
        .expect("gateway");
        let data_key = crate::DataKey::from_base64("fixture-credential", &BASE64.encode([9; 32]))
            .expect("测试密钥有效");
        let sealer = Arc::new(CredentialSealer::new(Arc::new(Keyring::single(
            DataKeyDomain::Credential,
            Arc::new(data_key),
        ))));
        let switches = RuntimeSwitches::default();
        let verifier =
            WorkerMailCredentialVerifier::new(sealer.clone(), Arc::new(gateway), switches.clone());
        Fixture { server, client, sealer, switches, verifier }
    }

    /// Admin 指定的 disabled 候选凭据：信封用候选 Cookie 密封，与运行 Cookie 不同。
    fn record(fixture: &Fixture, world: AdminCredentialWorld) -> AdminCredentialCheckRecord {
        let envelope = fixture
            .sealer
            .seal(
                AdminCredentialKind::MailCookie,
                CREDENTIAL,
                &world.scope(AdminCredentialKind::MailCookie),
                CANDIDATE_COOKIE.as_bytes(),
            )
            .expect("密封候选 Cookie");
        AdminCredentialCheckRecord {
            credential: AdminCredential {
                id: CREDENTIAL,
                kind: AdminCredentialKind::MailCookie,
                world,
                label: "backup".to_owned(),
                player_name: None,
                status: AdminCredentialStatus::Disabled,
                state_version: 3,
                created_at: Utc::now(),
                rotated_at: None,
                updated_at: Utc::now(),
            },
            envelope,
        }
    }

    fn observation(
        world: AdminCredentialWorld,
        status: AdminCredentialCheckStatus,
    ) -> AdminCredentialCheckObservation {
        AdminCredentialCheckObservation { world, status }
    }

    /// Worker 自己的一次常规读取（收件箱首页），走同一共享客户端。
    async fn worker_read(client: &MoogleMailClient) -> Result<Vec<u8>, MoogleMailErrorCategory> {
        let target = client.inbox_page(MoogleMailWorld::Main, 0).expect("inbox target");
        let cookie = MoogleMailCookie::new(ACTIVE_COOKIE).expect("active cookie");
        client.get(&target, &cookie).await.map_err(|error| error.category)
    }

    #[tokio::test]
    async fn specified_credential_is_probed_with_its_own_cookie() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::CookieInvalid));

        let checks = fixture
            .verifier
            .verify(record(&fixture, AdminCredentialWorld::Main))
            .await
            .expect("检查完成");

        // 登录失效是确定的 Invalid；请求只带被检查凭据自己的 Cookie，绝不改用运行 Cookie。
        assert_eq!(
            checks,
            vec![observation(AdminCredentialWorld::Main, AdminCredentialCheckStatus::Invalid)]
        );
        assert_eq!(fixture.server.request_cookies(), vec![Some(CANDIDATE_COOKIE.to_owned())]);
        assert_eq!(fixture.server.write_requests(), 0);
    }

    #[tokio::test]
    async fn busy_window_refuses_the_probe_without_network_or_using_the_window_probe() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::AccountBusy));
        assert_eq!(
            worker_read(&fixture.client).await.err(),
            Some(MoogleMailErrorCategory::AccountBusy)
        );
        let reads = fixture.server.read_requests();

        let checks = fixture
            .verifier
            .verify(record(&fixture, AdminCredentialWorld::Shared))
            .await
            .expect("检查完成");

        assert_eq!(
            checks,
            vec![
                observation(AdminCredentialWorld::Main, AdminCredentialCheckStatus::Inconclusive),
                observation(AdminCredentialWorld::Isekai, AdminCredentialCheckStatus::Inconclusive),
            ]
        );
        // main 处于账号忙窗口：候选检查不发网络；isekai 没有窗口，照常读取（替身返回战斗页）。
        assert_eq!(fixture.server.read_requests(), reads + 1);
        // 候选检查没有占用 main 窗口唯一的只读探测：Worker 自己的下一次读取仍可作为探测发出。
        let _ = worker_read(&fixture.client).await;
        assert_eq!(fixture.server.read_requests(), reads + 2);
    }

    #[tokio::test]
    async fn probe_waits_for_the_workers_in_flight_request_instead_of_overlapping() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::Timeout(Duration::from_millis(300))));
        let worker = worker_read(&fixture.client);
        let check = async {
            // Worker 的请求先进入在途状态，再发起候选检查。
            tokio::time::sleep(Duration::from_millis(50)).await;
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await
        };

        let (_, checks) = tokio::join!(worker, check);

        assert_eq!(
            checks.expect("检查完成"),
            vec![observation(AdminCredentialWorld::Main, AdminCredentialCheckStatus::Inconclusive)]
        );
        assert_eq!(fixture.server.read_requests(), 2);
        assert_eq!(fixture.server.max_concurrent_requests(), 1, "同一账号的物理请求不得重叠");
    }

    #[tokio::test]
    async fn overlapping_paused_or_non_mail_checks_never_reach_the_network() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::Timeout(Duration::from_millis(300))));
        let first = fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main));
        let second = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await
        };
        let (first, second) = tokio::join!(first, second);
        assert!(first.is_ok());
        assert!(
            matches!(
                second,
                Err(ApplicationError::AdminCredential(AdminCredentialError::CheckBusy))
            ),
            "重叠检查立即拒绝"
        );
        assert_eq!(fixture.server.read_requests(), 1);

        // 取消在途检查后许可与在途锁随之释放，下一次检查照常执行。
        let cancelled = tokio::time::timeout(
            Duration::from_millis(50),
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)),
        )
        .await;
        assert!(cancelled.is_err(), "检查在替身回应前被取消");
        assert!(
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await.is_ok()
        );

        let mut forum = record(&fixture, AdminCredentialWorld::Shared);
        forum.credential.kind = AdminCredentialKind::ForumCookie;
        assert!(matches!(
            fixture.verifier.verify(forum).await,
            Err(ApplicationError::AdminCredential(AdminCredentialError::InvalidRequest))
        ));

        let now = Utc::now();
        fixture
            .switches
            .install(
                RuntimeSwitchRead {
                    database_time: now,
                    events: vec![RuntimeSwitchEvent {
                        command_id: Uuid::new_v4(),
                        name: RuntimeSwitchName::ExternalAccessPaused,
                        revision: 1,
                        operation: RuntimeSwitchOperation::Set,
                        value: Some(serde_json::json!(true)),
                        expires_at: Some(now + TimeDelta::seconds(3_600)),
                        expired_command_id: None,
                        actor_id: "operator".to_owned(),
                        entrypoint: "cli".to_owned(),
                        reason: "drill".to_owned(),
                        recorded_at: now,
                    }],
                },
                std::time::Instant::now(),
            )
            .expect("暂停快照有效");
        let reads = fixture.server.read_requests();
        assert!(matches!(
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await,
            Err(ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable))
        ));
        assert_eq!(fixture.server.read_requests(), reads, "暂停期间不发起外部请求");
    }
}
