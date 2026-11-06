//! 扫描水位、持久检查点与已证明 Sent 摘要缓存；只影响增量读取，不参与投递证明。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::diagnostics as safe_diagnostics;

use auction_application::mail::{
    MailScanCheckpoint, MailScanCheckpointKey, MailScanCheckpointStore, MailScanEvent,
    MailScanObserver, MailScanStream, MailScanWatermark, advance_mail_scan_watermark,
};
use chrono::Utc;

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use super::{CachedReadScan, CachedSentBaseline, CachedSentScan, MoogleMailSender};

use crate::clients::mooglemail::{
    MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailWorld,
};

/// 已证明 Sent 基线的复用窗口。过期只会退回完整扫描，不改变任何证明。
const SENT_BASELINE_REUSE_WINDOW: Duration = Duration::from_secs(600);

impl MoogleMailSender {
    /// 记录扫描预算实际耗尽；只读同步指标，不改变截止点或重试。
    pub(super) fn observe_budget(
        &self,
        world: MoogleMailWorld,
        phase: auction_application::external_metrics::MailPhase,
        budget: auction_application::external_metrics::MailBudget,
    ) {
        self.client.external_metrics().observe(
            auction_application::external_metrics::ExternalMetricsEvent::MailBudgetExhausted {
                world: crate::clients::metrics::mail_world(world),
                phase,
                budget,
            },
        );
    }

    /// 从既有扫描错误闭集映射预算耗尽类别，不把普通读取失败算成预算。
    pub(super) fn observe_budget_error(
        &self,
        world: MoogleMailWorld,
        phase: auction_application::external_metrics::MailPhase,
        error: &MoogleMailError,
    ) {
        match error.category {
            MoogleMailErrorCategory::SentScanTimeout => {
                self.observe_budget(
                    world,
                    phase,
                    auction_application::external_metrics::MailBudget::Time,
                );
            }
            MoogleMailErrorCategory::SentScanPageLimit => {
                self.observe_budget(
                    world,
                    phase,
                    auction_application::external_metrics::MailBudget::Pages,
                );
            }
            _ => {}
        }
    }

    /// 复用最近一次完整证明的 Sent 摘要；凭据不同或超过窗口时必须重新扫描。
    /// 基线只是排除集：缺失条目最多增加候选详情读取，绝不放行未经验证的正文或资产。
    pub(crate) fn cached_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<HashSet<String>> {
        let cache = self
            .sent_baselines
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_baseline",
                    "lock_cached_baseline",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        let entry = cache.get(&world)?;
        (entry.credential == cookie.observation_key()
            && self
                .client
                .check_sent_scope(cookie, &entry.scope)
                .inspect_err(|cause| {
                    safe_diagnostics::debug(
                        "mooglemail_baseline",
                        "check_cached_scope",
                        safe_diagnostics::project(cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                })
                .is_ok()
            && entry.captured.elapsed() <= SENT_BASELINE_REUSE_WINDOW)
            .then(|| entry.seen.clone())
    }

    /// 由发送前完整枚举或发送后完整排除集与前缀的并集写入；单独水位不能充当完整缓存。
    pub(crate) fn remember_sent_baseline(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        seen: HashSet<String>,
    ) {
        let Ok(scope) = self.client.current_sent_scope(cookie, world).inspect_err(|cause| {
            safe_diagnostics::debug(
                "mooglemail_baseline",
                "read_cache_scope",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        if let Ok(mut cache) = self.sent_baselines.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_baseline",
                "lock_baseline_store",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) {
            cache.insert(
                world,
                CachedSentBaseline {
                    credential: cookie.observation_key(),
                    scope,
                    seen,
                    captured: Instant::now(),
                },
            );
        }
    }

    pub(crate) fn forget_sent_baseline(&self, world: MoogleMailWorld) {
        if let Ok(mut cache) = self.sent_baselines.lock().inspect_err(|cause| {
            safe_diagnostics::warn(
                "mooglemail_baseline",
                "lock_baseline_invalidation",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) {
            cache.remove(&world);
        }
    }

    /// 注入扫描观测；观察失败不改变任何证明或回退路径。
    pub fn with_mail_scan_observer(mut self, observer: Arc<dyn MailScanObserver>) -> Self {
        self.scan_observer = observer;
        self
    }

    pub(super) fn observe_scan(&self, world: MoogleMailWorld, event: MailScanEvent) {
        self.scan_observer.observe(world.as_str(), event);
    }

    /// 注入持久检查点；未注入时行为与只有进程内水位一致。
    pub fn with_mail_scan_checkpoint_store(
        mut self,
        store: Arc<dyn MailScanCheckpointStore>,
    ) -> Self {
        self.scan_checkpoints = Some(store);
        self
    }

    /// 只有数据库来源的 Cookie 才具备稳定凭据身份与版本；配置 Cookie 只使用内存。
    fn scan_checkpoint_key(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        stream: MailScanStream,
    ) -> Option<MailScanCheckpointKey> {
        let persisted = cookie.persisted_credential()?;
        let key = MailScanCheckpointKey {
            credential_id: persisted.id,
            credential_version: persisted.version,
            world_id: world.as_str().to_owned(),
            stream,
            endpoint_scope: self.client.endpoint_scope().to_owned(),
        };
        key.validate().then_some(key)
    }

    /// 读取持久检查点；缺失、损坏或读取失败都只意味着本轮不能增量。
    async fn load_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        stream: MailScanStream,
    ) -> Option<MailScanCheckpoint> {
        let store = self.scan_checkpoints.as_ref()?;
        let key = self.scan_checkpoint_key(cookie, world, stream)?;
        match store.load(&key).await {
            Ok(Some(checkpoint)) if checkpoint.validate() => Some(checkpoint),
            Ok(Some(_)) => {
                tracing::warn!(
                    world = world.as_str(),
                    stream = stream.label(),
                    "MoogleMail 持久扫描检查点无效；回退完整扫描"
                );
                None
            }
            Ok(None) => None,
            Err(error) => {
                let _ = error;
                tracing::debug!(
                    world = world.as_str(),
                    stream = stream.label(),
                    "MoogleMail 持久扫描检查点读取失败；回退完整扫描"
                );
                None
            }
        }
    }

    /// 重启后的首次 Sent 发送前加载持久检查点；已有有效内存条目时不覆盖。
    pub(super) async fn ensure_sent_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) {
        if self.sent_scan_checkpoint(cookie, world).is_some() {
            return;
        }
        let Some(checkpoint) = self.load_scan_checkpoint(cookie, world, MailScanStream::Sent).await
        else {
            return;
        };
        let Ok(scope) = self.client.current_sent_scope(cookie, world) else {
            return;
        };
        let Ok(mut cache) = self.sent_scans.lock() else {
            return;
        };
        let reuse_existing = cache.get(&world).is_some_and(|entry| {
            entry.credential == cookie.observation_key()
                && self.client.check_sent_scope(cookie, &entry.scope).is_ok()
        });
        if !reuse_existing {
            cache.insert(
                world,
                CachedSentScan { credential: cookie.observation_key(), scope, checkpoint },
            );
        }
    }

    /// Read Mail 维护轮加载持久检查点；它与投递游标完全独立。
    pub(super) async fn ensure_read_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) {
        if self.read_scan_checkpoint(cookie, world).is_some() {
            return;
        }
        let Some(checkpoint) =
            self.load_scan_checkpoint(cookie, world, MailScanStream::ReadMail).await
        else {
            return;
        };
        let Ok(mut cache) = self.read_scans.lock() else {
            return;
        };
        if cache.get(&world).is_none_or(|entry| entry.credential != cookie.observation_key()) {
            cache
                .insert(world, CachedReadScan { credential: cookie.observation_key(), checkpoint });
        }
    }

    /// 持久化失败只影响下次重启优化；当前轮已确认的水位仍保留在内存。
    async fn persist_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        stream: MailScanStream,
        checkpoint: &MailScanCheckpoint,
    ) {
        let Some(store) = self.scan_checkpoints.as_ref() else {
            return;
        };
        let Some(key) = self.scan_checkpoint_key(cookie, world, stream) else {
            return;
        };
        if store.save(&key, checkpoint).await.is_err() {
            tracing::debug!(
                world = world.as_str(),
                stream = stream.label(),
                "MoogleMail 持久扫描检查点写入失败；不影响本轮结果"
            );
        }
    }

    /// 只有凭据与读取代次仍一致的水位才可用于下一轮前缀。
    pub(crate) fn sent_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<MailScanCheckpoint> {
        let cache = self
            .sent_scans
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_sent_scan",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        let entry = cache.get(&world)?;
        (entry.credential == cookie.observation_key()
            && self
                .client
                .check_sent_scope(cookie, &entry.scope)
                .inspect_err(|cause| {
                    safe_diagnostics::debug(
                        "mooglemail_scan",
                        "check_sent_scan_scope",
                        safe_diagnostics::project(cause),
                        DiagnosticContext {
                            world: diagnostic_world(world.as_str()),
                            ..Default::default()
                        },
                    );
                })
                .is_ok())
        .then(|| entry.checkpoint.clone())
    }

    /// 记录 Sent 水位；`full` 才推进每日校验时间。水位缺失或作用域变化时只会回退完整扫描。
    pub(crate) async fn remember_sent_scan(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        newest: Option<MailScanWatermark>,
        full: bool,
    ) {
        let Ok(scope) = self.client.current_sent_scope(cookie, world).inspect_err(|cause| {
            safe_diagnostics::debug(
                "mooglemail_scan",
                "read_sent_scan_scope",
                safe_diagnostics::project(cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
        }) else {
            return;
        };
        // 锁只覆盖内存水位更新；持久化必须在释放 guard 后执行。
        let checkpoint = {
            let Ok(mut cache) = self.sent_scans.lock().inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_sent_scan_store",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            }) else {
                return;
            };
            let previous = cache.get(&world).filter(|entry| {
                entry.credential == cookie.observation_key()
                    && self.client.check_sent_scope(cookie, &entry.scope).is_ok()
            });
            let watermark = newest
                .map(|newest| {
                    advance_mail_scan_watermark(
                        previous.and_then(|entry| entry.checkpoint.watermark.as_ref()),
                        &newest,
                    )
                })
                .or_else(|| previous.and_then(|entry| entry.checkpoint.watermark.clone()));
            let last_full_scan_at = if full {
                Some(Utc::now())
            } else {
                previous.and_then(|entry| entry.checkpoint.last_full_scan_at)
            };
            let checkpoint = MailScanCheckpoint { watermark, last_full_scan_at };
            cache.insert(
                world,
                CachedSentScan {
                    credential: cookie.observation_key(),
                    scope,
                    checkpoint: checkpoint.clone(),
                },
            );
            checkpoint
        };
        self.persist_scan_checkpoint(cookie, world, MailScanStream::Sent, &checkpoint).await;
    }

    /// Read Mail 水位只绑定凭据；维护观察轮次不携带 Sent 作用域。
    pub(crate) fn read_scan_checkpoint(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
    ) -> Option<MailScanCheckpoint> {
        let cache = self
            .read_scans
            .lock()
            .inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_read_scan",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            })
            .ok()?;
        cache
            .get(&world)
            .filter(|entry| entry.credential == cookie.observation_key())
            .map(|entry| entry.checkpoint.clone())
    }

    pub(crate) async fn remember_read_scan(
        &self,
        cookie: &MoogleMailCookie,
        world: MoogleMailWorld,
        newest: Option<MailScanWatermark>,
        full: bool,
    ) {
        let checkpoint = {
            let Ok(mut cache) = self.read_scans.lock().inspect_err(|cause| {
                safe_diagnostics::warn(
                    "mooglemail_scan",
                    "lock_read_scan_store",
                    safe_diagnostics::project(cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
            }) else {
                return;
            };
            let previous =
                cache.get(&world).filter(|entry| entry.credential == cookie.observation_key());
            let watermark = newest
                .map(|newest| {
                    advance_mail_scan_watermark(
                        previous.and_then(|entry| entry.checkpoint.watermark.as_ref()),
                        &newest,
                    )
                })
                .or_else(|| previous.and_then(|entry| entry.checkpoint.watermark.clone()));
            let last_full_scan_at = if full {
                Some(Utc::now())
            } else {
                previous.and_then(|entry| entry.checkpoint.last_full_scan_at)
            };
            let checkpoint = MailScanCheckpoint { watermark, last_full_scan_at };
            cache.insert(
                world,
                CachedReadScan {
                    credential: cookie.observation_key(),
                    checkpoint: checkpoint.clone(),
                },
            );
            checkpoint
        };
        self.persist_scan_checkpoint(cookie, world, MailScanStream::ReadMail, &checkpoint).await;
    }
}
