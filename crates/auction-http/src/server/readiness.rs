//! HTTP 最终就绪判定的短缓存与单飞；不缓存数据库数据，不参与业务命令准入。

use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};

use auction_application::auction::ApplicationFuture;
use auction_application::core::RepositoryError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::operations::ProcessReadinessPort;
use auction_observability::{ReadinessCacheOutcome, ReadinessCheckResult, ReadinessRecorder};
use futures_util::FutureExt;
use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

use crate::diagnostics;

const CACHE_TTL: Duration = Duration::from_secs(1);
const CHECK_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Clone, Copy)]
struct CachedResult {
    ready: bool,
    completed_at: Instant,
}

type RefreshReceiver = watch::Receiver<Option<CachedResult>>;

/// 装配根包装完整的角色 readiness；心跳仍直接读取原始端口。
pub struct HttpReadinessProbe {
    source: Arc<dyn ProcessReadinessPort>,
    switches: auction_application::runtime_switches::RuntimeSwitches,
    current: Mutex<
        Option<(auction_application::runtime_switches::RuntimeReadinessPolicy, RefreshReceiver)>,
    >,
    metrics: Arc<dyn ReadinessRecorder>,
}

impl HttpReadinessProbe {
    /// 注入最终角色判定、启动开关和指标；所有 state 克隆应共享同一个探针实例。
    pub fn new(
        source: Arc<dyn ProcessReadinessPort>,
        cache_enabled: bool,
        metrics: Arc<dyn ReadinessRecorder>,
    ) -> Self {
        Self {
            source,
            switches: auction_application::runtime_switches::RuntimeSwitches::new(
                auction_application::runtime_switches::RuntimeSwitchRole::Admin,
                auction_application::operations::AuditFailureMode::default(),
                cache_enabled,
            ),
            current: Mutex::new(None),
            metrics,
        }
    }

    /// 与存储、心跳和指标共享动态策略；身份包含最新版本及到期状态。
    pub fn with_runtime_switches(
        mut self,
        switches: auction_application::runtime_switches::RuntimeSwitches,
    ) -> Self {
        self.switches = switches;
        self
    }

    async fn cached_ready(
        &self,
        policy: auction_application::runtime_switches::RuntimeReadinessPolicy,
    ) -> bool {
        let mut current = self.current.lock().await;
        if let Some((cached_policy, receiver)) = &*current
            && *cached_policy == policy
        {
            let cached = *receiver.borrow();
            if let Some(cached) = cached {
                if cached.completed_at.elapsed() < CACHE_TTL {
                    self.metrics.cache_request(ReadinessCacheOutcome::Hit);
                    return cached.ready;
                }
            } else {
                let receiver = receiver.clone();
                self.metrics.cache_request(ReadinessCacheOutcome::Coalesced);
                drop(current);
                return wait_for_refresh(receiver).await;
            }
        }
        let (sender, receiver) = watch::channel(None);
        *current = Some((policy, receiver.clone()));
        self.metrics.cache_request(ReadinessCacheOutcome::Refresh);
        // 守卫在 spawn 前创建：即使任务在首次 poll 前被取消，也会发布失败并结束单飞。
        let completion = RefreshCompletion {
            sender: Some(sender),
            observation: CheckObservation::new(Arc::clone(&self.metrics)),
        };
        let source = Arc::clone(&self.source);
        let diagnostic_context = diagnostics::RequestDiagnostics::capture();
        let body = async move { refresh(source, completion).await };
        tokio::spawn(diagnostic_context.run(body));
        drop(current);
        wait_for_refresh(receiver).await
    }
}

impl ProcessReadinessPort for HttpReadinessProbe {
    fn check(&self) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            let policy = self.switches.readiness_policy();
            if !policy.cache_enabled {
                self.metrics.cache_request(ReadinessCacheOutcome::Bypass);
                let mut observation = CheckObservation::new(Arc::clone(&self.metrics));
                let result = self.source.check().await;
                if let Err(error) = &result {
                    diagnostics::warn(
                        "http_readiness",
                        "check_source_direct",
                        error.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                }
                observation.result = if result.is_ok() {
                    ReadinessCheckResult::Ready
                } else {
                    ReadinessCheckResult::NotReady
                };
                if self.switches.readiness_policy() != policy {
                    return Err(
                        RepositoryError::Failed("就绪策略已切换，请重试探针".to_owned()).into()
                    );
                }
                return result;
            }
            // 旧策略任务仅发布到自己的 channel；改变策略或期限后不能复用其缓存或成功结果。
            let ready = self.cached_ready(policy).await;
            if ready && self.switches.readiness_policy() == policy {
                Ok(())
            } else {
                Err(RepositoryError::Failed("进程 readiness 检查失败".to_owned()).into())
            }
        })
    }
}

async fn wait_for_refresh(mut receiver: RefreshReceiver) -> bool {
    loop {
        if let Some(result) = *receiver.borrow_and_update() {
            return result.ready;
        }
        if receiver.changed().await.is_err() {
            return false;
        }
    }
}

async fn refresh(source: Arc<dyn ProcessReadinessPort>, mut completion: RefreshCompletion) {
    let result =
        AssertUnwindSafe(async { tokio::time::timeout(CHECK_TIMEOUT, source.check()).await })
            .catch_unwind()
            .await;
    let (ready, outcome) = match result {
        Ok(Ok(Ok(()))) => (true, ReadinessCheckResult::Ready),
        Ok(Err(timeout)) => {
            // 截止前没有完整依赖证明，超时按未就绪处理；只保留类型，不记录错误原文。
            diagnostics::warn(
                "http_readiness",
                "check_source_timeout",
                diagnostics::timeout_kind(&timeout),
                DiagnosticContext::default(),
            );
            let _elapsed = timeout;
            (false, ReadinessCheckResult::Timeout)
        }
        Ok(Ok(Err(error))) => {
            diagnostics::warn(
                "http_readiness",
                "check_source_refresh",
                error.diagnostic_kind(),
                DiagnosticContext::default(),
            );
            (false, ReadinessCheckResult::NotReady)
        }
        Err(panic_payload) => {
            // panic 无法证明依赖就绪；载荷可能含敏感值，不格式化也不写入日志。
            diagnostics::error(
                "http_readiness",
                "check_source_panic",
                diagnostics::panic_kind(&panic_payload),
                DiagnosticContext::default(),
            );
            drop(panic_payload);
            (false, ReadinessCheckResult::NotReady)
        }
    };
    completion.observation.result = outcome;
    completion.publish(ready);
}

struct CheckObservation {
    metrics: Arc<dyn ReadinessRecorder>,
    started: Instant,
    result: ReadinessCheckResult,
}

impl CheckObservation {
    fn new(metrics: Arc<dyn ReadinessRecorder>) -> Self {
        Self { metrics, started: Instant::now(), result: ReadinessCheckResult::NotReady }
    }
}

impl Drop for CheckObservation {
    fn drop(&mut self) {
        self.metrics.check_finished(self.result, self.started.elapsed());
    }
}

struct RefreshCompletion {
    sender: Option<watch::Sender<Option<CachedResult>>>,
    observation: CheckObservation,
}

impl RefreshCompletion {
    fn publish(&mut self, ready: bool) {
        if let Some(sender) = self.sender.take() {
            sender.send_replace(Some(CachedResult { ready, completed_at: Instant::now() }));
        }
    }
}

impl Drop for RefreshCompletion {
    fn drop(&mut self) {
        // 取消或异常不能遗留 None 状态；负结果也缓存一秒，故障期匿名请求不会逐次查库。
        self.publish(false);
    }
}
