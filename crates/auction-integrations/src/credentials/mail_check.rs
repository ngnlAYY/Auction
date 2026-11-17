//! Worker 侧的邮件凭据在线检查：Admin 经 Worker control 转来的检查在本进程按 ID 打开数据库信封，
//! 经与邮件循环共用的 gateway 以候选身份读取余额页。被检查的可以是 disabled/invalid 凭据，绝不以
//! 当前运行 Cookie 代替；不借用运行凭据会话、不改变凭据状态，成功也不结束当前账号的忙碌期。
//!
//! 每个世界的探测分两段。发出前的排队（请求门、在途锁、账号忙与 owner 复核）在检查自己的 future
//! 内执行，检查超时或被取消即撤销票据与锁登记退出，不在后台补发。取得在途锁、完成 owner 复核之后、
//! 真正发出之前直接复核主动暂停；通过这道边界的探测交给独立任务走完，在途锁持有到远端回应或客户端
//! 超时，检查名额也随它保留到结束，因此后台探测只属于当前唯一被接纳的检查，不随反复取消累积。客户端
//! 超时只说明 Worker 放弃了这次请求，不能证明远端已停止处理。主动暂停另在入口、每个世界开始前与
//! 等待期间复核。

use std::{sync::Arc, time::Duration};

use auction_application::admin::{
    AdminCredentialCheckObservation, AdminCredentialCheckRecord, AdminCredentialCheckStatus,
    AdminCredentialError, AdminCredentialKind, AdminCredentialOpener, AdminCredentialVerifier,
    AdminCredentialWorld, credential_check_worlds,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};
use auction_application::runtime_switches::RuntimeSwitches;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::MissedTickBehavior;

use super::DecryptedCookie;
use crate::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailGateway, MoogleMailWorld,
};
use crate::safe_diagnostics::{self, ErrorProjection};

/// 单个世界的探测预算，含请求门与在途锁排队；shared 凭据最多两个世界，合计仍在转发预算内。
const WORLD_CHECK_BUDGET: Duration = Duration::from_secs(5);
/// 等待探测期间复核主动暂停的间隔；暂停快照按自身刷新周期更新，这里只缩短排队中的反应时间。
/// 发出前另有一次不依赖轮询的直接复核。
const PAUSE_RECHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Worker 进程内执行的邮件 Cookie 探测器；只接受数据库信封，只检查邮件 Cookie。
pub struct WorkerMailCredentialVerifier {
    opener: Arc<dyn AdminCredentialOpener>,
    gateway: Arc<MoogleMailGateway>,
    switches: RuntimeSwitches,
    /// 唯一的检查名额：检查返回且其已发出的探测全部走完才归还。
    slots: Arc<Semaphore>,
    world_budget: Duration,
    pause_recheck: Duration,
}

/// 单个世界探测的结局。
enum WorldProbe {
    /// 得到结论，含无法得出结论。
    Checked(AdminCredentialCheckStatus),
    /// 等待期间或发出前主动暂停生效：尚未发出的探测不再发出（已发出的在后台走完），整次检查按
    /// 暂不可用返回。
    Paused,
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
            slots: Arc::new(Semaphore::new(1)),
            world_budget: WORLD_CHECK_BUDGET,
            pause_recheck: PAUSE_RECHECK_INTERVAL,
        }
    }

    /// 测试缩短单个世界的探测预算，覆盖预算到期后的串行。
    #[cfg(test)]
    fn with_world_budget(mut self, world_budget: Duration) -> Self {
        self.world_budget = world_budget;
        self
    }

    /// 测试拉长暂停轮询间隔：等待期间不再轮询，暂停只能由发出边界的直接复核拦下。
    #[cfg(test)]
    fn with_pause_recheck(mut self, pause_recheck: Duration) -> Self {
        self.pause_recheck = pause_recheck;
        self
    }

    async fn check_world(
        &self,
        world: AdminCredentialWorld,
        secret: &[u8],
        slot: &Arc<OwnedSemaphorePermit>,
    ) -> WorldProbe {
        use AdminCredentialCheckStatus::Inconclusive;
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
        let mut probe = std::pin::pin!(self.probe(world, &cookie, slot));
        // 调用方刚在本世界开始前复核过暂停，首次轮询放在一个间隔之后，不立即重复。
        let mut pause_check = tokio::time::interval_at(
            tokio::time::Instant::now() + self.pause_recheck,
            self.pause_recheck,
        );
        pause_check.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                outcome = &mut probe => return outcome,
                _ = pause_check.tick() => {
                    // 等待期间暂停生效：丢弃探测 future 即撤销排队中的票据与锁登记；已发出的探测
                    // 仍在后台走完。
                    if self.switches.external_access_paused() {
                        return WorldProbe::Paused;
                    }
                }
            }
        }
    }

    /// 单个世界的两段式探测：排队在本 future 内，发出交给带着检查名额的独立任务。
    async fn probe(
        &self,
        world: MoogleMailWorld,
        cookie: &MoogleMailCookie,
        slot: &Arc<OwnedSemaphorePermit>,
    ) -> WorldProbe {
        // Item Shop 只读余额页不触发邮件打开、取附件、compose、send 或战斗操作。Valid 只表示当次
        // Cookie 可用；余额不是自身名称证据，账号身份只由内部组合证据端口联合证明。
        // 排队阶段被取消或预算到期即随本 future 撤票退出，不会在无人等待时补发。
        let queued = match self.gateway.queue_candidate_probe(world, cookie).await {
            Ok(queued) => queued,
            Err(error) => return conclude(world, Err(error)),
        };
        // 发出边界：已持有在途锁并完成 owner 复核。轮询读到未暂停之后、在途锁释放之前生效的暂停只有
        // 在这里才能拦下；复核与下面的 spawn 之间没有等待点，结论即发出决定。拦下时丢弃探测，在途锁
        // 随之释放。
        if self.switches.external_access_paused() {
            return WorldProbe::Paused;
        }
        // 已发出的探测在独立任务中走完：本 future 被取消或预算到期只放弃等待，任务持有在途锁到远端
        // 回应或客户端超时，并带着检查名额直到结束。
        let slot = Arc::clone(slot);
        let exchange = tokio::spawn(async move {
            let _slot = slot;
            queued.send().await
        });
        match exchange.await {
            Ok(result) => conclude(world, result),
            Err(error) => inconclusive(world, safe_diagnostics::project(&error)),
        }
    }
}

/// 把探测结果折算为单个世界的结论：成功为 Valid，登录失效为 Invalid，其余无法得出结论。
fn conclude(world: MoogleMailWorld, result: Result<(), MoogleMailError>) -> WorldProbe {
    match result {
        Ok(()) => WorldProbe::Checked(AdminCredentialCheckStatus::Valid),
        Err(error) if error.category == MoogleMailErrorCategory::SessionExpired => {
            WorldProbe::Checked(AdminCredentialCheckStatus::Invalid)
        }
        Err(error) => inconclusive(world, safe_diagnostics::project(&error)),
    }
}

/// 无法得出结论：只记一条脱敏诊断，不回显 Cookie 或页面。
fn inconclusive(world: MoogleMailWorld, cause: ErrorProjection) -> WorldProbe {
    safe_diagnostics::warn(
        "credential_check",
        "verify_game_session",
        cause,
        DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
    );
    WorldProbe::Checked(AdminCredentialCheckStatus::Inconclusive)
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
            // 立即拒绝重叠检查：同一账号的物理请求本就串行，排队只会挤占 Worker 自己的读写。已发出的
            // 探测带着名额直到走完，上一次检查的探测仍在远端处理时同样拒绝。
            let slot = Arc::new(
                Arc::clone(&self.slots)
                    .try_acquire_owned()
                    .ok()
                    .ok_or(AdminCredentialError::CheckBusy)?,
            );
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
                let probe = tokio::time::timeout(
                    self.world_budget,
                    self.check_world(*world, &secret.0, &slot),
                )
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
        gateway: Arc<MoogleMailGateway>,
        sealer: Arc<CredentialSealer>,
        switches: RuntimeSwitches,
        verifier: WorkerMailCredentialVerifier,
    }

    impl Fixture {
        /// 与主检查器共用 gateway 与暂停开关、只缩短单个世界预算的另一检查器。
        fn verifier_with_world_budget(&self, budget: Duration) -> WorkerMailCredentialVerifier {
            WorkerMailCredentialVerifier::new(
                self.sealer.clone(),
                Arc::clone(&self.gateway),
                self.switches.clone(),
            )
            .with_world_budget(budget)
        }
    }

    async fn fixture() -> Fixture {
        fixture_with_interval(Duration::from_millis(1)).await
    }

    /// `interval` 为共享请求门的最小间隔。
    async fn fixture_with_interval(interval: Duration) -> Fixture {
        let server = MailDouble::start().await.expect("loopback server");
        let client = Arc::new(
            MoogleMailClient::with_config(MoogleMailClientConfig {
                endpoint: server.endpoint(),
                maximum_response_bytes: 8 * 1024 * 1024,
                minimum_request_interval: interval,
                request_timeout: Duration::from_secs(5),
            })
            .expect("loopback client"),
        );
        let gateway = Arc::new(
            MoogleMailGateway::new(
                Arc::clone(&client),
                MoogleMailCredentialSet::new(ACTIVE_COOKIE).expect("active cookie"),
                MoogleMailGatewayConfig::default(),
            )
            .expect("gateway"),
        );
        let data_key = crate::DataKey::from_base64("fixture-credential", &BASE64.encode([9; 32]))
            .expect("测试密钥有效");
        let sealer = Arc::new(CredentialSealer::new(Arc::new(Keyring::single(
            DataKeyDomain::Credential,
            Arc::new(data_key),
        ))));
        let switches = RuntimeSwitches::default();
        let verifier = WorkerMailCredentialVerifier::new(
            sealer.clone(),
            Arc::clone(&gateway),
            switches.clone(),
        );
        Fixture { server, client, gateway, sealer, switches, verifier }
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

    fn check_busy(result: &Result<Vec<AdminCredentialCheckObservation>, ApplicationError>) -> bool {
        matches!(result, Err(ApplicationError::AdminCredential(AdminCredentialError::CheckBusy)))
    }

    /// 轮询等待测试观察到的状态成立；5 秒内不成立即失败。
    async fn wait_until(condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("观察条件应在 5 秒内成立");
    }

    /// 交替用调用方取消与单个世界预算到期，放弃四次仍在发出前排队的 main 检查。
    async fn abandon_queued_checks(
        fixture: &Fixture,
        budget_limited: &WorkerMailCredentialVerifier,
    ) {
        for round in 0..4 {
            if round % 2 == 0 {
                let cancelled = tokio::time::timeout(
                    Duration::from_millis(30),
                    fixture.verifier.verify(record(fixture, AdminCredentialWorld::Main)),
                )
                .await;
                assert!(cancelled.is_err(), "第 {round} 轮：检查在排队中被调用方取消");
            } else {
                assert_eq!(
                    budget_limited
                        .verify(record(fixture, AdminCredentialWorld::Main))
                        .await
                        .expect("检查完成"),
                    vec![observation(
                        AdminCredentialWorld::Main,
                        AdminCredentialCheckStatus::Inconclusive
                    )],
                    "第 {round} 轮：排队中预算到期只说明这次没有结论"
                );
            }
        }
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

    /// 已发出的探测把检查名额带进后台：调用方取消后、远端回应前，新的检查立即返回 busy，不进入
    /// 请求门或在途锁；探测走完才归还名额，此后的检查照常执行。后台候选探测因此只属于一个检查，
    /// 不会随反复取消累积。
    #[tokio::test]
    async fn sent_probe_keeps_the_check_slot_until_it_finishes() {
        let fixture = fixture().await;
        fixture.server.set_fault(Some(Fault::DetachedDelay(Duration::from_millis(400))));
        let cancelled = tokio::time::timeout(
            Duration::from_millis(100),
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)),
        )
        .await;
        assert!(cancelled.is_err(), "检查在远端回应前被调用方取消");
        assert_eq!(fixture.server.request_cookies(), vec![Some(CANDIDATE_COOKIE.to_owned())]);

        for attempt in 0..3 {
            let refused =
                fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await;
            assert!(
                check_busy(&refused),
                "第 {attempt} 次：已发出的探测未结束时立即拒绝：{refused:?}"
            );
        }
        assert_eq!(fixture.client.queued_waiters(), (0, 0), "被拒绝的检查不排队");

        // 远端处理完、后台探测结束后名额归还；此前只会得到 busy。
        fixture.server.set_fault(None);
        let admitted = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let result =
                    fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await;
                if !check_busy(&result) {
                    break result;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("后台探测走完后名额归还");
        assert_eq!(
            admitted.expect("检查完成"),
            vec![observation(AdminCredentialWorld::Main, AdminCredentialCheckStatus::Inconclusive)]
        );
        assert_eq!(
            fixture.server.request_cookies(),
            vec![Some(CANDIDATE_COOKIE.to_owned()); 2],
            "被拒绝的检查没有发出探测"
        );
        assert_eq!(fixture.server.max_concurrent_requests(), 1, "同一账号的物理请求不得重叠");
    }

    /// 发出前在请求门上排队的探测随调用方取消或单个世界预算到期撤票：反复超时/取消之后门上不留
    /// 失效票据，Worker 紧接的读取按正常间隔放行，远端从未收到候选探测。
    #[tokio::test]
    async fn repeated_queued_timeouts_and_cancellations_leave_no_stale_gate_tickets() {
        let interval = Duration::from_millis(400);
        let fixture = fixture_with_interval(interval).await;
        let budget_limited = fixture.verifier_with_world_budget(Duration::from_millis(30));
        // Worker 刚用掉令牌：之后的候选探测都要在门上等下一个时间槽。
        worker_read(&fixture.client).await.expect("Worker 读取成功");

        abandon_queued_checks(&fixture, &budget_limited).await;

        assert_eq!(fixture.client.queued_waiters(), (0, 0), "请求门与在途锁不得留下失效登记");
        let started = std::time::Instant::now();
        worker_read(&fixture.client).await.expect("Worker 仍能前进");
        assert!(
            started.elapsed() < interval * 2,
            "Worker 读取被失效票据推迟：{:?}",
            started.elapsed()
        );
        assert_eq!(
            fixture.server.request_cookies(),
            vec![Some(ACTIVE_COOKIE.to_owned()); 2],
            "候选探测从未发出"
        );
    }

    /// 发出前在在途锁上排队的探测同样随取消或预算到期退出：Worker 的请求挂在远端期间反复超时/
    /// 取消，锁上不留失效登记；Worker 请求结束后紧接的读取直接取得锁，远端从未收到候选探测。
    #[tokio::test]
    async fn checks_abandoned_behind_a_worker_request_leave_no_stale_lock_waiters() {
        let fixture = fixture().await;
        let budget_limited = fixture.verifier_with_world_budget(Duration::from_millis(30));
        let hold = fixture.server.hold_responses();
        let worker = worker_read(&fixture.client);
        let abandon = async {
            // Worker 的请求已到达远端并挂起，在途锁由它持有。
            wait_until(|| fixture.server.requests().len() == 1).await;
            abandon_queued_checks(&fixture, &budget_limited).await;
            let waiters = fixture.client.queued_waiters();
            hold.release();
            waiters
        };

        let (first, waiters) = tokio::join!(worker, abandon);

        first.expect("Worker 读取成功");
        assert_eq!(waiters, (0, 0), "请求门与在途锁不得留下失效登记");
        worker_read(&fixture.client).await.expect("Worker 仍能前进");
        assert_eq!(
            fixture.server.request_cookies(),
            vec![Some(ACTIVE_COOKIE.to_owned()); 2],
            "候选探测从未发出"
        );
        assert_eq!(fixture.server.max_concurrent_requests(), 1);
    }

    /// 发出边界直接复核暂停：世界开始前的复核读到未暂停，候选探测在 Worker 的在途请求后排队；暂停
    /// 随后生效，Worker 请求再结束释放在途锁。排队的探测取得锁并完成 owner 复核后，必须在发出前被
    /// 拦下：整次检查暂不可用，远端只收到 Worker 自己的请求。轮询间隔拉长到一小时，等待期间没有
    /// 轮询，结果不依赖轮询时机。
    #[tokio::test]
    async fn pause_after_the_last_poll_is_caught_at_the_send_boundary() {
        let mut fixture = fixture().await;
        fixture.verifier = fixture.verifier.with_pause_recheck(Duration::from_secs(3_600));
        let hold = fixture.server.hold_responses();
        let worker = worker_read(&fixture.client);
        let check = async {
            wait_until(|| fixture.server.requests().len() == 1).await;
            fixture.verifier.verify(record(&fixture, AdminCredentialWorld::Main)).await
        };
        let pause_then_release = async {
            // 候选探测已在在途锁上排队：世界开始前的复核早已读到未暂停。
            wait_until(|| fixture.client.queued_waiters() == (0, 1)).await;
            pause_external_access(&fixture.switches);
            hold.release();
        };

        let (worker, checks, ()) = tokio::join!(worker, check, pause_then_release);

        worker.expect("Worker 读取成功");
        assert!(check_unavailable(&checks), "发出前复核到暂停，整次检查暂不可用：{checks:?}");
        assert_eq!(
            fixture.server.request_cookies(),
            vec![Some(ACTIVE_COOKIE.to_owned())],
            "暂停生效后不得发出候选探测"
        );
        assert_eq!(fixture.client.queued_waiters(), (0, 0));
    }

    /// 检查被调用方取消或单个世界预算到期，都只放弃等待：已发出的探测在独立任务里走完，在途锁持有
    /// 到远端回应为止。替身不随客户端断开停止处理，紧接着的 Worker 请求必须排在它之后，远端看到的
    /// 同一账号物理请求不重叠。替身延迟（600ms）短于客户端超时（5 秒）；远端处理超过客户端超时后
    /// Worker 放弃请求、释放在途锁，远端是否已停止无从证明，不在本测试范围。
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
