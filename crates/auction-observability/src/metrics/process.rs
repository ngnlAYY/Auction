//! 进程级 Prometheus 注册表与导出边界。

use std::sync::Arc;

use prometheus::{
    Encoder, Gauge, GaugeVec, Opts, Registry, TextEncoder, process_collector::ProcessCollector,
};

use super::{
    audit::{AuditMetricsCollector, AuditMetricsSource},
    business::PrometheusBusinessRecorder,
    http::HttpRecorder,
    migration::{self, MigrationReadinessRecorder},
    pool::PoolMetricsCollector,
    readiness::{self, ReadinessRecorder},
    tracing::{DisabledTracingHealth, TracingCollector, valid_identity},
    types::{
        BusinessRecorder, MetricsError, PoolMetricsSource, WorkerRecorder, noop_business_recorder,
        noop_worker_recorder,
    },
    worker::PrometheusWorkerRecorder,
};
use crate::health::TracingHealthReader;

pub struct ProcessMetrics {
    shutdown: Arc<dyn super::shutdown::ShutdownRecorder>,
    enabled: bool,
    registry: Registry,
    worker: Arc<dyn WorkerRecorder>,
    business: Arc<dyn BusinessRecorder>,
    readiness: Arc<dyn ReadinessRecorder>,
    migration_readiness: MigrationReadinessRecorder,
    sent_baseline: Arc<dyn auction_application::SentBaselineObserver>,
    external_metrics: Arc<dyn auction_application::ExternalMetricsObserver>,
    shadow: Arc<dyn auction_application::ShadowRecorder>,
    http: HttpRecorder,
    database: super::DatabaseRecorder,
    notifications: Arc<dyn auction_application::NotificationRecorder>,
}

impl ProcessMetrics {
    /// 附加观测模块只在启用时注册 collector，不暴露注册表给业务层。
    pub(super) fn enabled_registry(&self) -> Option<&Registry> {
        self.enabled.then_some(&self.registry)
    }

    /// 注册同源纯内存开关指标；关闭 exporter 不影响刷新。
    pub fn register_runtime_switch_metrics(
        &self,
        switches: auction_application::RuntimeSwitches,
    ) -> Result<(), MetricsError> {
        if !self.enabled {
            return Ok(());
        }
        self.registry
            .register(Box::new(super::runtime_switches::RuntimeSwitchCollector::new(switches)?))
            .map_err(MetricsError::Registry)
    }

    /// 装配根注册一次进程审计快照；关闭 exporter 不关闭审计失败计数。
    pub fn register_audit_metrics(
        &self,
        source: Arc<dyn AuditMetricsSource>,
    ) -> Result<(), MetricsError> {
        if !self.enabled {
            return Ok(());
        }
        self.registry
            .register(Box::new(AuditMetricsCollector::new(source)?))
            .map_err(MetricsError::Registry)
    }

    /// 创建进程指标注册表。关闭时不注册 collector，也不会建立后台任务。
    pub fn new(
        enabled: bool,
        service: &'static str,
        role: &'static str,
        version: &'static str,
    ) -> Result<Self, MetricsError> {
        Self::new_with_tracing_health(
            enabled,
            service,
            role,
            version,
            Arc::new(DisabledTracingHealth),
        )
    }

    /// 创建带 tracing 安全状态读取器的进程指标注册表。
    /// tracing 指标只读取固定状态字段，不把 exporter endpoint、headers 或 span 内容暴露给
    /// Prometheus；Public/Worker 没有 exporter 时传入的关闭读取器仍会产生稳定的零值族。
    pub fn new_with_tracing_health(
        enabled: bool,
        service: &'static str,
        role: &'static str,
        version: &'static str,
        tracing_health: Arc<dyn TracingHealthReader>,
    ) -> Result<Self, MetricsError> {
        if !enabled {
            return Ok(Self::disabled());
        }
        if !valid_identity(service) || !valid_identity(role) || !valid_identity(version) {
            return Err(MetricsError::Registry(prometheus::Error::Msg(
                "metrics identity is invalid".to_owned(),
            )));
        }

        let registry = Registry::new();
        let process_up = Gauge::with_opts(Opts::new(
            "auction_process_up",
            "Whether the process has completed telemetry initialization.",
        ))
        .map_err(MetricsError::Registry)?;
        process_up.set(1.0);
        registry.register(Box::new(process_up)).map_err(MetricsError::Registry)?;

        let metrics_enabled = Gauge::with_opts(Opts::new(
            "auction_metrics_enabled",
            "Whether the process-local Prometheus exporter is enabled.",
        ))
        .map_err(MetricsError::Registry)?;
        metrics_enabled.set(1.0);
        registry.register(Box::new(metrics_enabled)).map_err(MetricsError::Registry)?;

        let process_info = GaugeVec::new(
            Opts::new("auction_process_info", "Static process identity information."),
            &["service", "role", "version"],
        )
        .map_err(MetricsError::Registry)?;
        process_info.with_label_values(&[service, role, version]).set(1.0);
        registry.register(Box::new(process_info)).map_err(MetricsError::Registry)?;

        // 旧版 registry 同时提供 CPU、内存、文件描述符、线程数和启动时间等进程资源指标。
        // ProcessCollector 只读取当前进程，不带连接串、请求参数或业务主键，适合直接复用。
        registry
            .register(Box::new(ProcessCollector::for_self()))
            .map_err(MetricsError::Registry)?;

        let tracing = TracingCollector::new(tracing_health)?;
        registry.register(Box::new(tracing)).map_err(MetricsError::Registry)?;

        let business = PrometheusBusinessRecorder::build(&registry)?;
        // W14.8 通知生命周期、失败、送达延迟与邮资估算；三角色共用同一闭集族。
        let notifications = super::notification::build(&registry)?;
        let worker = if role == "worker" {
            PrometheusWorkerRecorder::build(&registry, Arc::clone(&business))?
        } else {
            noop_worker_recorder()
        };

        let readiness = readiness::build(&registry)?;
        let migration_readiness = migration::build(&registry)?;
        let sent_baseline = if role == "worker" {
            super::sent_baseline::build(&registry)?
        } else {
            auction_application::noop_sent_baseline_observer()
        };
        let external_metrics = if role == "worker" {
            super::external_client::build(&registry)?
        } else {
            auction_application::noop_external_metrics_observer()
        };
        // W0.15 影子运行框架的共享计数；三角色都预置零值，未接入 owner 时保持为空。
        let shadow = super::shadow_run::build(&registry)?;
        let shutdown = super::shutdown::build(&registry)?;
        let http = HttpRecorder::build(&registry, role)?;
        let database = super::DatabaseRecorder::build(&registry)?;
        Ok(Self {
            enabled: true,
            registry,
            worker,
            business,
            readiness,
            migration_readiness,
            sent_baseline,
            external_metrics,
            shadow,
            shutdown,
            http,
            database,
            notifications,
        })
    }

    /// 创建空注册表与 no-op 记录器；关闭时不注册 collector，也不建立后台任务。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            registry: Registry::new(),
            worker: noop_worker_recorder(),
            business: noop_business_recorder(),
            readiness: readiness::noop(),
            migration_readiness: MigrationReadinessRecorder::default(),
            sent_baseline: auction_application::noop_sent_baseline_observer(),
            external_metrics: auction_application::noop_external_metrics_observer(),
            shadow: super::shadow_run::noop(),
            shutdown: super::shutdown::noop(),
            http: HttpRecorder::default(),
            database: super::DatabaseRecorder::default(),
            notifications: auction_application::noop_notification_recorder(),
        }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// 以 Prometheus text exposition format 生成一次完整快照。
    pub fn encode(&self) -> Result<Vec<u8>, MetricsError> {
        if !self.enabled {
            return Err(MetricsError::Disabled);
        }
        let families = self.registry.gather();
        let mut output = Vec::new();
        TextEncoder::new().encode(&families, &mut output).map_err(MetricsError::Encode)?;
        Ok(output)
    }

    /// 返回 scrape 响应的稳定 Content-Type。
    pub const fn content_type() -> &'static str {
        "text/plain; version=0.0.4; charset=utf-8"
    }

    /// 返回当前进程的 Worker 指标端口；Public/Admin 会得到 no-op 实现。
    pub fn worker(&self) -> Arc<dyn WorkerRecorder> {
        Arc::clone(&self.worker)
    }

    pub fn business(&self) -> Arc<dyn BusinessRecorder> {
        Arc::clone(&self.business)
    }

    /// 返回 W14.8 通知指标端口；关闭指标时为 no-op。
    pub fn notifications(&self) -> Arc<dyn auction_application::NotificationRecorder> {
        Arc::clone(&self.notifications)
    }

    /// 返回仅接受闭集身份的排空指标端口。
    pub fn shutdown(&self) -> Arc<dyn super::shutdown::ShutdownRecorder> {
        Arc::clone(&self.shutdown)
    }

    /// 返回 HTTP readiness 的闭集记录器；由角色装配根注入探针。
    pub fn readiness(&self) -> Arc<dyn ReadinessRecorder> {
        Arc::clone(&self.readiness)
    }

    /// 返回迁移就绪的闭集记录器；数据库检查与 heartbeat 共用同一进程计数。
    pub fn migration_readiness(&self) -> MigrationReadinessRecorder {
        self.migration_readiness.clone()
    }

    /// 返回 Worker 的发送基线局部影子记录器；其他角色及关闭指标时为 no-op。
    pub fn sent_baseline(&self) -> Arc<dyn auction_application::SentBaselineObserver> {
        Arc::clone(&self.sent_baseline)
    }

    /// 返回 W0.15 影子运行共享记录端口；关闭指标时为 no-op。
    /// 新旧判定的裁决权属于调用点，记录器只累加样本与差异。
    pub fn shadow(&self) -> Arc<dyn auction_application::ShadowRecorder> {
        Arc::clone(&self.shadow)
    }

    /// 返回外部论坛与邮件客户端的闭集指标端口；关闭指标时为 no-op。
    pub fn external_metrics(&self) -> Arc<dyn auction_application::ExternalMetricsObserver> {
        Arc::clone(&self.external_metrics)
    }

    /// 返回 Public/Admin 共用的 HTTP 记录器；关闭指标或其他角色得到 no-op。
    pub fn http(&self) -> HttpRecorder {
        self.http.clone()
    }

    /// 返回附加数据库观测端口；关闭时不参与业务判定。
    pub fn database(&self) -> super::DatabaseRecorder {
        self.database.clone()
    }

    /// 注册数据库连接池状态 collector。
    /// 数据库通常在 HTTP/Worker 装配中晚于进程指标对象建立，因此允许在连接成功后注册一次。
    /// collector 只读取池的内存快照，不执行 SQL，也不把连接串或数据库身份带入指标。
    pub fn register_pool_metrics(
        &self,
        source: Arc<dyn PoolMetricsSource>,
    ) -> Result<(), MetricsError> {
        if !self.enabled {
            return Ok(());
        }
        self.registry
            .register(Box::new(PoolMetricsCollector::new(source)?))
            .map_err(MetricsError::Registry)
    }
}
