//! 同源只读快照、TOML 基线与期限解析；缓存仅选择都正确的运行路径。

use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};

use super::{
    RuntimeSwitchError, RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchOperation,
    RuntimeSwitchRead, RuntimeSwitchRole,
};
use crate::AuditFailureMode;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

/// 单次就绪检查绑定的策略身份；期限失效也改变身份，使旧缓存失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeReadinessPolicy {
    /// 原分类累计的投影模式。
    pub audit_failure_mode: AuditFailureMode,
    /// 是否使用 HTTP 短缓存。
    pub cache_enabled: bool,
    revisions: [i64; 2],
    overrides: [bool; 2],
}

#[derive(Debug)]
struct Snapshot {
    read: RuntimeSwitchRead,
    loaded_at: Option<Instant>,
}

/// 一个名称的安全展示，同时用于固定标签指标。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSwitchView {
    /// 已知名称。
    pub name: RuntimeSwitchName,
    /// 本次数据库读取或最后成功刷新中的最新事实。
    pub latest: Option<RuntimeSwitchEvent>,
    /// 进程启动时 TOML 值。
    pub toml_baseline: Value,
    /// 当前角色实际解析值。
    pub effective_value: Value,
    /// 已观察版本；零表示尚无事实。
    pub applied_revision: i64,
    /// 当前值是否来自未过期覆盖。
    pub overridden: bool,
    /// 此角色是否消费该项。
    pub applies: bool,
}

/// 本进程实际快照；最后成功年龄为空表示尚无成功读取。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSwitchStatus {
    /// 固定角色。
    pub role: RuntimeSwitchRole,
    /// 最后成功刷新年龄，读取失败不更新。
    pub last_success_age_seconds: Option<f64>,
    /// 成功刷新累计。
    pub refresh_successes: u64,
    /// 失败刷新累计。
    pub refresh_failures: u64,
    /// 本版全部已知开关。
    pub switches: Vec<RuntimeSwitchView>,
}

/// 三角色各自共享的不可变快照句柄；无 SQL、配置加载或运行时依赖。
#[derive(Debug, Clone)]
pub struct RuntimeSwitches {
    role: RuntimeSwitchRole,
    audit_baseline: AuditFailureMode,
    cache_baseline: bool,
    sli_baseline: bool,
    state: Arc<RwLock<Arc<Snapshot>>>,
    successes: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
}

impl Default for RuntimeSwitches {
    fn default() -> Self {
        Self::new(RuntimeSwitchRole::Worker, AuditFailureMode::default(), true)
    }
}

impl RuntimeSwitches {
    /// 装配根注入当前角色经过配置 owner 校验的 TOML 基线。
    pub fn new(
        role: RuntimeSwitchRole,
        audit_baseline: AuditFailureMode,
        cache_baseline: bool,
    ) -> Self {
        Self {
            role,
            audit_baseline,
            cache_baseline,
            sli_baseline: false,
            state: Arc::new(RwLock::new(Arc::new(Snapshot {
                read: RuntimeSwitchRead { database_time: Utc::now(), events: Vec::new() },
                loaded_at: None,
            }))),
            successes: Arc::new(AtomicU64::new(0)),
            failures: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Worker 在首次刷新前注入已校验的指标汇总基线。
    pub fn with_sli_snapshot_baseline(mut self, enabled: bool) -> Self {
        self.sli_baseline = enabled;
        self
    }

    /// 仅 Worker 消费；沿用同一最新事实、期限与失败回落规则。
    pub fn sli_snapshot_enabled(&self) -> bool {
        self.status().switches.into_iter().any(|view| {
            view.name == RuntimeSwitchName::MetricsSliSnapshotEnabled
                && view.applies
                && view.effective_value.as_bool().unwrap_or(false)
        })
    }

    fn current(&self) -> Arc<Snapshot> {
        // 锁内只有完整 Arc 替换；异常不会暴露半构造快照。
        match self.state.read() {
            Ok(state) => Arc::clone(&state),
            Err(poison) => {
                record_error(
                    DiagnosticLevel::Warn,
                    poison.diagnostic_kind(),
                    "runtime_switches",
                    "recover_snapshot_read",
                    DiagnosticContext::default(),
                );
                Arc::clone(&poison.into_inner())
            }
        }
    }

    /// 全量校验后原子安装；调用方传入查询开始时刻，网络延迟不会延长覆盖期限。
    pub fn install(
        &self,
        read: RuntimeSwitchRead,
        started_at: Instant,
    ) -> Result<(), RuntimeSwitchError> {
        for (index, event) in read.events.iter().enumerate() {
            event.validate()?;
            if read.events[..index].iter().any(|previous| previous.name == event.name) {
                return Err(RuntimeSwitchError::Corrupt);
            }
        }
        let snapshot = Arc::new(Snapshot { read, loaded_at: Some(started_at) });
        match self.state.write() {
            Ok(mut state) => *state = snapshot,
            Err(poison) => {
                record_error(
                    DiagnosticLevel::Warn,
                    poison.diagnostic_kind(),
                    "runtime_switches",
                    "recover_snapshot_write",
                    DiagnosticContext::default(),
                );
                *poison.into_inner() = snapshot;
            }
        }
        increment(&self.successes);
        Ok(())
    }

    /// 读取失败保持原快照及其期限，首次失败自然保留 TOML 基线。
    pub fn refresh_failed(&self) {
        increment(&self.failures);
    }

    /// 每次使用均重新判定最新事实的期限，不从更早事实恢复 override。
    pub fn status(&self) -> RuntimeSwitchStatus {
        let snapshot = self.current();
        let age = snapshot.loaded_at.map(|at| at.elapsed());
        let now = age
            .and_then(|age| chrono::Duration::from_std(age).ok())
            .and_then(|age| snapshot.read.database_time.checked_add_signed(age));
        let switches = RuntimeSwitchName::ALL
            .into_iter()
            .map(|name| {
                let latest = snapshot.read.events.iter().find(|event| event.name == name).cloned();
                let applies = name.applies_to(self.role);
                let overridden = applies
                    && latest.as_ref().is_some_and(|event| {
                        event.operation == RuntimeSwitchOperation::Set
                            && event
                                .expires_at
                                .is_none_or(|until| now.is_some_and(|now| now < until))
                    });
                let baseline = match name {
                    RuntimeSwitchName::ReadinessAuditFailureMode => {
                        json!(audit_label(self.audit_baseline))
                    }
                    RuntimeSwitchName::ReadinessCacheEnabled => json!(self.cache_baseline),
                    RuntimeSwitchName::MetricsSliSnapshotEnabled => json!(self.sli_baseline),
                };
                let effective_value = if overridden {
                    latest
                        .as_ref()
                        .and_then(|event| event.value.clone())
                        .unwrap_or_else(|| baseline.clone())
                } else {
                    baseline.clone()
                };
                RuntimeSwitchView {
                    name,
                    applied_revision: latest.as_ref().map_or(0, |event| event.revision),
                    latest,
                    toml_baseline: baseline,
                    effective_value,
                    overridden,
                    applies,
                }
            })
            .collect();
        RuntimeSwitchStatus {
            role: self.role,
            last_success_age_seconds: age.map(|age| age.as_secs_f64()),
            refresh_successes: self.successes.load(Ordering::Relaxed),
            refresh_failures: self.failures.load(Ordering::Relaxed),
            switches,
        }
    }

    /// 固定一次检查所使用的策略；相同版本在到期前保持相同身份。
    pub fn readiness_policy(&self) -> RuntimeReadinessPolicy {
        let status = self.status();
        let mut policy = RuntimeReadinessPolicy {
            audit_failure_mode: self.audit_baseline,
            cache_enabled: self.cache_baseline,
            revisions: [0; 2],
            overrides: [false; 2],
        };
        for view in &status.switches {
            // SLI 开关不改变 HTTP 就绪缓存身份，也不能进入两项策略数组。
            let index = match view.name {
                RuntimeSwitchName::ReadinessAuditFailureMode => 0,
                RuntimeSwitchName::ReadinessCacheEnabled => 1,
                RuntimeSwitchName::MetricsSliSnapshotEnabled => continue,
            };
            policy.revisions[index] = view.applied_revision;
            policy.overrides[index] = view.overridden;
            match view.name {
                RuntimeSwitchName::ReadinessAuditFailureMode => {
                    policy.audit_failure_mode = match view.effective_value.as_str() {
                        Some("legacy") => AuditFailureMode::Legacy,
                        Some("classified") => AuditFailureMode::Classified,
                        _ => AuditFailureMode::Shadow,
                    }
                }
                RuntimeSwitchName::ReadinessCacheEnabled => {
                    policy.cache_enabled =
                        view.effective_value.as_bool().unwrap_or(self.cache_baseline)
                }
                RuntimeSwitchName::MetricsSliSnapshotEnabled => {}
            }
        }
        policy
    }
}

fn increment(counter: &AtomicU64) {
    // 饱和累计不因成功刷新清零，也不回绕。
    #[allow(deprecated)]
    let _ = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| Some(value.saturating_add(1)));
}

/// 审计模式的稳定字符串，配置、指标和安全展示共用。
pub const fn audit_label(mode: AuditFailureMode) -> &'static str {
    match mode {
        AuditFailureMode::Legacy => "legacy",
        AuditFailureMode::Shadow => "shadow",
        AuditFailureMode::Classified => "classified",
    }
}
