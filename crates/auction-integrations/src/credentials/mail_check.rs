//! Worker 侧的邮件凭据在线检查：Admin 经 Worker control 转来的检查在本进程按 ID 打开数据库信封，
//! 经与邮件循环共用的 gateway 以候选身份读取余额页。被检查的可以是 disabled/invalid 凭据，绝不以
//! 当前运行 Cookie 代替；不借用运行凭据会话、不改变凭据状态，成功也不结束当前账号的忙碌期。
//!
//! 每个世界的探测在独立任务中执行：检查超时或被取消只放弃等待，已发出的物理请求照常走完，在途锁
//! 持有到远端回应或客户端超时，紧接着的 Worker 请求不会与远端仍在处理的探测重叠；尚未发出的探测
//! 由候选 Cookie 的撤销标记拦下。主动暂停在入口、每个世界开始前以及排队等待期间复核，生效后不再
//! 发出新的探测。

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
use tokio::time::MissedTickBehavior;

use super::DecryptedCookie;
use crate::mooglemail::{
    MoogleMailCookie, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailWorld,
};
use crate::safe_diagnostics;

/// 单个世界的探测预算，含请求门与在途锁排队；shared 凭据最多两个世界，合计仍在转发预算内。
const WORLD_CHECK_BUDGET: Duration = Duration::from_secs(5);
/// 等待探测期间复核主动暂停的间隔；暂停快照按自身刷新周期更新，这里只缩短排队中的反应时间。
const PAUSE_RECHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Worker 进程内执行的邮件 Cookie 探测器；只接受数据库信封，只检查邮件 Cookie。
pub struct WorkerMailCredentialVerifier {
    opener: Arc<dyn AdminCredentialOpener>,
    gateway: Arc<MoogleMailGateway>,
    switches: RuntimeSwitches,
    slots: Semaphore,
    world_budget: Duration,
}

/// 单个世界探测的结局。
enum WorldProbe {
    /// 得到结论，含无法得出结论。
    Checked(AdminCredentialCheckStatus),
    /// 等待期间主动暂停生效：尚未发出的探测已撤销，整次检查按暂不可用返回。
    Paused,
}

/// 候选 Cookie 的放行门：客户端在取得在途锁之后、真正发出请求之前复核撤销标记。检查得出结论、
/// 预算到期、被取消或遇到暂停时随本值析构而撤销，排队中的探测不再发出；已发出的探测不受影响，
/// 在独立任务中走完。
struct ProbeAdmission(MoogleMailCookie);

impl Drop for ProbeAdmission {
    fn drop(&mut self) {
        self.0.revoke();
    }
}

impl WorkerMailCredentialVerifier {
    /// `gateway` 必须是本进程邮件循环使用的实例，探测才与 Worker 自己的读写共用请求门、在途锁、
    /// 账号忙熔断与运行 owner；`switches` 提供主动暂停外部访问的开关。
    pub fn new(
        opener: Arc<dyn AdminCredentialOpener>,
        gateway: Arc<MoogleMailGateway>,
        switches: RuntimeSwitches,
    ) -> Self {
        Self {
            opener,
            gateway,
            switches,
            slots: Semaphore::new(1),
            world_budget: WORLD_CHECK_BUDGET,
        }
    }

    /// 测试缩短单个世界的探测预算，覆盖预算到期后的串行。
    #[cfg(test)]
    fn with_world_budget(mut self, world_budget: Duration) -> Self {
        self.world_budget = world_budget;
        self
    }

    async fn check_world(&self, world: AdminCredentialWorld, secret: &[u8]) -> WorldProbe {
        use AdminCredentialCheckStatus::{Inconclusive, Invalid, Valid};
        let world = match world {
            AdminCredentialWorld::Main => MoogleMailWorld::Main,
            AdminCredentialWorld::Isekai => MoogleMailWorld::Isekai,
            AdminCredentialWorld::Shared => return WorldProbe::Checked(Inconclusive),
        };
        let Ok(secret) = std::str::from_utf8(secret) else {
            tracing::debug!(
                world = world.as_str(),
                reason = "not_utf8",
                "邮件凭据检查无法得出结论"
            );
            return WorldProbe::Checked(Inconclusive);
        };
        let Ok(cookie) = MoogleMailCookie::new(secret) else {
            tracing::debug!(
                world = world.as_str(),
                reason = "invalid_cookie",
                "邮件凭据检查无法得出结论"
            );
            return WorldProbe::Checked(Inconclusive);
        };
        let _admission = ProbeAdmission(cookie.clone());
        // Item Shop 只读余额页不触发邮件打开、取附件、compose、send 或战斗操作。Valid 只表示当次
        // Cookie 可用；余额不是自身名称证据，账号身份只由内部组合证据端口联合证明。
        // 在独立任务中探测：本 future 被取消或预算到期只放弃等待，已发出的请求持有在途锁直到走完。
        let gateway = Arc::clone(&self.gateway);
        let mut probe =
            tokio::spawn(async move { gateway.probe_candidate_cookie(world, &cookie).await });
        let mut pause_check = tokio::time::interval(PAUSE_RECHECK_INTERVAL);
        pause_check.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let result = loop {
            tokio::select! {
                result = &mut probe => break result,
                _ = pause_check.tick() => {
                    // 排队期间暂停生效：返回即撤销放行门，排队中的探测不再发出。
                    if self.switches.external_access_paused() {
                        return WorldProbe::Paused;
                    }
                }
            }
        };
        let diagnostic_cause = match result {
            Ok(Ok(())) => return WorldProbe::Checked(Valid),
            Ok(Err(error)) if error.category == MoogleMailErrorCategory::SessionExpired => {
                return WorldProbe::Checked(Invalid);
            }
            Ok(Err(error)) => safe_diagnostics::project(&error),
            Err(error) => safe_diagnostics::project(&error),
        };
        safe_diagnostics::warn(
            "credential_check",
            "verify_game_session",
            diagnostic_cause,
            DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
        );
        WorldProbe::Checked(Inconclusive)
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
                // 两个世界之间生效的暂停同样不再发起新的探测。
                if self.switches.external_access_paused() {
                    return Err(AdminCredentialError::CheckUnavailable.into());
                }
                // 预算含排队；超时只说明这次没有结论，不重试。
                let probe =
                    tokio::time::timeout(self.world_budget, self.check_world(*world, &secret.0))
                        .await
                        .unwrap_or(WorldProbe::Checked(AdminCredentialCheckStatus::Inconclusive));
                let status = match probe {
                    WorldProbe::Checked(status) => status,
                    WorldProbe::Paused => return Err(AdminCredentialError::CheckUnavailable.into()),
                };
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

    /// 安装一小时的主动暂停外部访问快照。
    fn pause_external_access(switches: &RuntimeSwitches) {
        let now = Utc::now();
        switches
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
    }

    fn check_unavailable(
        result: &Result<Vec<AdminCredentialCheckObservation>, ApplicationError>,
    ) -> bool {
        matches!(
            result,
            Err(ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable))
        )
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

        // 取消在途检查后许可随之释放；已发出的探测在后台走完才释放在途锁，下一次检查随后照常执行。
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

        pause_external_access(&fixture.switches);
        let reads = fixture.server.read_requests();
        assert!(check_unavailable(
            &fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await
        ));
        assert_eq!(fixture.server.read_requests(), reads, "暂停期间不发起外部请求");
    }

    /// 检查被调用方取消或单个世界预算到期，都只放弃等待：已发出的探测在独立任务里走完，在途锁持有
    /// 到远端回应为止。替身不随客户端断开停止处理，紧接着的 Worker 请求必须排在它之后，远端看到的
    /// 同一账号物理请求不重叠。
    #[tokio::test]
    async fn cancelled_or_timed_out_probe_keeps_the_next_worker_request_serialized() {
        for budget_expires in [false, true] {
            let mut fixture = fixture().await;
            if budget_expires {
                fixture.verifier = fixture.verifier.with_world_budget(Duration::from_millis(150));
            }
            fixture.server.set_fault(Some(Fault::DetachedDelay(Duration::from_millis(600))));
            let checked = record(&fixture, AdminCredentialWorld::Main);
            if budget_expires {
                assert_eq!(
                    fixture.verifier.verify(checked).await.expect("检查完成"),
                    vec![observation(
                        AdminCredentialWorld::Main,
                        AdminCredentialCheckStatus::Inconclusive
                    )],
                    "预算到期只说明这次没有结论"
                );
            } else {
                let cancelled = tokio::time::timeout(
                    Duration::from_millis(150),
                    fixture.verifier.verify(checked),
                )
                .await;
                assert!(cancelled.is_err(), "检查在远端回应前被调用方取消");
            }

            let _ = worker_read(&fixture.client).await;

            assert_eq!(
                fixture.server.request_cookies(),
                vec![Some(CANDIDATE_COOKIE.to_owned()), Some(ACTIVE_COOKIE.to_owned())],
                "budget_expires={budget_expires}"
            );
            assert_eq!(
                fixture.server.max_concurrent_requests(),
                1,
                "budget_expires={budget_expires}：紧接的 Worker 请求不得与远端仍在处理的探测重叠"
            );
        }
    }

    /// 候选检查通过入口后在 Worker 的在途请求后排队，暂停在排队期间生效：整次检查暂不可用，在途锁
    /// 释放后排队的探测也不再发出，远端只收到 Worker 自己的请求。
    #[tokio::test]
    async fn pause_while_queued_sends_no_candidate_probe() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::Timeout(Duration::from_millis(600))));
        let worker = worker_read(&fixture.client);
        let check = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await
        };
        let pause = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            pause_external_access(&fixture.switches);
        };

        let (_, checks, ()) = tokio::join!(worker, check, pause);

        assert!(check_unavailable(&checks), "排队期间暂停生效后整次检查暂不可用");
        // 在途锁已释放：再等一段时间，确认排队的探测没有在暂停后补发。
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            fixture.server.request_cookies(),
            vec![Some(ACTIVE_COOKIE.to_owned())],
            "暂停后不得发出候选探测"
        );
    }

    /// shared 凭据的 main 探测已发出、isekai 尚未发起时暂停：main 的探测在后台照常结束，isekai 不再
    /// 发起，整次检查暂不可用。
    #[tokio::test]
    async fn pause_between_worlds_sends_no_second_probe() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::Timeout(Duration::from_millis(800))));
        let check = fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Shared));
        let pause = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            pause_external_access(&fixture.switches);
        };

        let (checks, ()) = tokio::join!(check, pause);

        assert!(check_unavailable(&checks), "两个世界之间暂停生效后整次检查暂不可用");
        // 等 main 的探测在后台走完，确认之后也没有补发 isekai。
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        let requests = fixture.server.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(!requests[0].contains("/isekai/"), "{requests:?}");
    }
}
