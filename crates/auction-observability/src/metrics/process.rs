//! 进程级 Prometheus 注册表与导出边界。

use std::sync::Arc;

use prometheus::{
    Encoder, Gauge, GaugeVec, Opts, Registry, TextEncoder, process_collector::ProcessCollector,
};

use super::{
    business::PrometheusBusinessRecorder,
    pool::PoolMetricsCollector,
    tracing::{DisabledTracingHealth, TracingCollector, valid_identity},
    types::{
        BusinessRecorder, MetricsError, PoolMetricsSource, WorkerRecorder, noop_business_recorder,
        noop_worker_recorder,
    },
    worker::PrometheusWorkerRecorder,
};
use crate::health::TracingHealthReader;

pub struct ProcessMetrics {
    enabled: bool,
    registry: Registry,
    worker: Arc<dyn WorkerRecorder>,
    business: Arc<dyn BusinessRecorder>,
}

impl ProcessMetrics {
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
        let worker = if role == "worker" {
            PrometheusWorkerRecorder::build(&registry, Arc::clone(&business))?
        } else {
            noop_worker_recorder()
        };

        Ok(Self { enabled: true, registry, worker, business })
    }

    /// 创建空注册表与 no-op 记录器；关闭时不注册 collector，也不建立后台任务。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            registry: Registry::new(),
            worker: noop_worker_recorder(),
            business: noop_business_recorder(),
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
