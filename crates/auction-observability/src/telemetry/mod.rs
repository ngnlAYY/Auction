use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, AtomicU8, Ordering},
    },
    time::Duration,
};

use crate::health::{TracingHealth, TracingHealthReader, TracingStatus};
use chrono::{DateTime, Utc};
use opentelemetry::{
    Context, KeyValue,
    propagation::TextMapPropagator,
    trace::{TraceContextExt, TracerProvider as _},
};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    propagation::TraceContextPropagator,
    trace::{
        Sampler, SdkTracerProvider, Span, SpanData, SpanExporter as SdkSpanExporter, SpanProcessor,
    },
};
use thiserror::Error;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// 把经过 HTTP 边界筛选的 W3C trace context 设为当前 server span 的远端父级。
///
/// 无效或超长字段会被忽略；函数不记录 header 原文，避免把不受信任的传播数据带入日志。
pub fn attach_remote_trace_context(
    span: &tracing::Span,
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) {
    let Some(traceparent) = traceparent.filter(|value| value.len() <= 512) else {
        return;
    };
    let mut carrier = HashMap::with_capacity(2);
    carrier.insert("traceparent".to_owned(), traceparent.to_owned());
    if let Some(tracestate) = tracestate.filter(|value| value.len() <= 1024) {
        carrier.insert("tracestate".to_owned(), tracestate.to_owned());
    }
    let context = TraceContextPropagator::new().extract(&carrier);
    if context.span().span_context().is_valid() {
        let _ = span.set_parent(context);
    }
}

/// tracing 导出所需的非敏感运行参数。
///
/// 端点只允许由上层配置模型校验后的 HTTP(S) 地址提供；本结构不承载请求 headers、Cookie
/// 或其他凭据，避免把秘密材料带入可观测性边界。headers 只在初始化时短暂传入 exporter。
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// 是否启用 OTLP/HTTP 导出。
    pub enabled: bool,
    /// OTLP/HTTP collector 地址；导出器会按 OTLP 约定补充 `/v1/traces`。
    pub endpoint: String,
    /// 批次等待时间。
    pub batch_timeout: Duration,
    /// 单次导出超时。
    pub export_timeout: Duration,
    /// 导出队列容量。
    pub queue_capacity: usize,
    /// 单批最大 span 数量。
    pub max_export_batch_size: usize,
    /// 父级采样比例，范围为 `0.0..=1.0`。
    pub sample_ratio: f64,
    /// 进程关闭时等待 exporter 完成的最大时间。
    pub shutdown_timeout: Duration,
}

#[derive(Debug, Clone, Copy)]
enum FailureCode {
    None = 0,
    Transport = 1,
    Shutdown = 2,
}

impl FailureCode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Transport => "transport",
            Self::Shutdown => "shutdown",
        }
    }
}

/// tracing exporter 的进程内安全状态；只保存可进入心跳的计数与闭集字段。
#[derive(Debug)]
struct TracingHealthState {
    enabled: bool,
    status: AtomicU8,
    dropped_span_count: AtomicI64,
    export_failure_count: AtomicI64,
    last_success_micros: AtomicI64,
    last_failure_micros: AtomicI64,
    last_failure_code: AtomicU8,
}

impl TracingHealthState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            status: AtomicU8::new(if enabled { 1 } else { 0 }),
            dropped_span_count: AtomicI64::new(0),
            export_failure_count: AtomicI64::new(0),
            last_success_micros: AtomicI64::new(0),
            last_failure_micros: AtomicI64::new(0),
            last_failure_code: AtomicU8::new(FailureCode::None as u8),
        }
    }

    fn record_success(&self) {
        self.last_success_micros.store(Utc::now().timestamp_micros(), Ordering::Relaxed);
        self.last_failure_code.store(FailureCode::None as u8, Ordering::Relaxed);
        self.status.store(1, Ordering::Release);
    }

    fn record_failure(&self, code: FailureCode) {
        self.export_failure_count.fetch_add(1, Ordering::Relaxed);
        self.last_failure_micros.store(Utc::now().timestamp_micros(), Ordering::Relaxed);
        self.last_failure_code.store(code as u8, Ordering::Relaxed);
        self.status.store(2, Ordering::Release);
    }

    fn snapshot(&self) -> TracingHealth {
        let status = match self.status.load(Ordering::Acquire) {
            1 => TracingStatus::Ok,
            2 => TracingStatus::Degraded,
            _ => TracingStatus::Disabled,
        };
        let failure_code = match self.last_failure_code.load(Ordering::Relaxed) {
            value if value == FailureCode::Transport as u8 => FailureCode::Transport,
            value if value == FailureCode::Shutdown as u8 => FailureCode::Shutdown,
            _ => FailureCode::None,
        };
        TracingHealth {
            status,
            dropped_span_count: self.dropped_span_count.load(Ordering::Relaxed),
            export_failure_count: self.export_failure_count.load(Ordering::Relaxed),
            last_success_at: timestamp(self.last_success_micros.load(Ordering::Relaxed)),
            last_failure_at: timestamp(self.last_failure_micros.load(Ordering::Relaxed)),
            last_error_code: if self.enabled {
                failure_code.as_str().to_owned()
            } else {
                String::new()
            },
        }
    }
}

impl TracingHealthReader for TracingHealthState {
    fn snapshot(&self) -> TracingHealth {
        TracingHealthState::snapshot(self)
    }
}

fn timestamp(value: i64) -> Option<DateTime<Utc>> {
    (value != 0).then(|| DateTime::from_timestamp_micros(value)).flatten()
}

/// 在不改变 OTLP exporter 行为的前提下记录安全的成功/失败结果。
#[derive(Debug)]
struct HealthSpanExporter<E> {
    exporter: E,
    health: Arc<TracingHealthState>,
}

impl<E> SdkSpanExporter for HealthSpanExporter<E>
where
    E: SdkSpanExporter,
{
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        let health = Arc::clone(&self.health);
        let export = self.exporter.export(batch);
        async move {
            match export.await {
                Ok(()) => {
                    health.record_success();
                    Ok(())
                }
                Err(error) => {
                    health.record_failure(FailureCode::Transport);
                    Err(error)
                }
            }
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        let result = self.exporter.force_flush();
        if result.is_err() {
            self.health.record_failure(FailureCode::Transport);
        }
        result
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let result = self.exporter.shutdown_with_timeout(timeout);
        if result.is_err() {
            self.health.record_failure(FailureCode::Shutdown);
        }
        result
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.exporter.set_resource(resource);
    }
}

/// 有界 OTLP 批处理器的控制消息。
///
/// Span 使用独立的有界 channel，控制消息使用无界 channel，确保 flush/shutdown 不会因为
/// span 队列已满而永远无法送达。
enum ProcessorControl {
    Flush(std::sync::mpsc::Sender<OTelSdkResult>),
    Shutdown(std::sync::mpsc::Sender<OTelSdkResult>, Duration),
    SetResource(Resource),
}

/// 能准确记录队列满导致丢弃数量的异步 span processor。
///
/// OpenTelemetry SDK 的内置 processor 不公开丢弃计数；这里保留相同的有界批处理语义，但在
/// `try_send` 失败的同步路径直接增加进程安全计数，避免用 exporter 失败数冒充队列丢弃数。
struct DropAwareBatchSpanProcessor<E> {
    span_sender: tokio::sync::mpsc::Sender<SpanData>,
    control_sender: tokio::sync::mpsc::UnboundedSender<ProcessorControl>,
    _exporter: std::marker::PhantomData<fn() -> E>,
    health: Arc<TracingHealthState>,
    state: AtomicU8,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<E> std::fmt::Debug for DropAwareBatchSpanProcessor<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DropAwareBatchSpanProcessor")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<E> DropAwareBatchSpanProcessor<E>
where
    E: SdkSpanExporter + 'static,
{
    fn new(
        exporter: E,
        health: Arc<TracingHealthState>,
        queue_capacity: usize,
        max_export_batch_size: usize,
        batch_timeout: Duration,
    ) -> Result<Self, InitError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| InitError {
            message: "OTLP exporter requires an active Tokio runtime".to_owned(),
        })?;
        let (span_sender, span_receiver) = tokio::sync::mpsc::channel(queue_capacity);
        let (control_sender, control_receiver) = tokio::sync::mpsc::unbounded_channel();
        let exporter = Arc::new(tokio::sync::Mutex::new(exporter));
        let worker_exporter = Arc::clone(&exporter);
        let worker = runtime.spawn(run_batch_processor(
            span_receiver,
            control_receiver,
            worker_exporter,
            max_export_batch_size,
            batch_timeout,
        ));

        Ok(Self {
            span_sender,
            control_sender,
            _exporter: std::marker::PhantomData,
            health,
            state: AtomicU8::new(0),
            worker: Mutex::new(Some(worker)),
        })
    }

    fn processor_error(message: impl Into<String>) -> OTelSdkError {
        OTelSdkError::InternalFailure(message.into())
    }

    fn abort_worker(&self) {
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            worker.abort();
        }
    }
}

impl<E> SpanProcessor for DropAwareBatchSpanProcessor<E>
where
    E: SdkSpanExporter + 'static,
{
    fn on_start(&self, _span: &mut Span, _cx: &Context) {}

    fn on_end(&self, span: SpanData) {
        if !span.span_context.is_sampled() {
            return;
        }
        if self.state.load(Ordering::Acquire) != 0 {
            self.health.dropped_span_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.span_sender.try_send(span).is_err() {
            self.health.dropped_span_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        if self.state.load(Ordering::Acquire) != 0 {
            return Err(Self::processor_error("OTLP span processor is shutting down"));
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.control_sender
            .send(ProcessorControl::Flush(sender))
            .map_err(|_| Self::processor_error("OTLP span processor queue is closed"))?;
        receiver
            .recv()
            .map_err(|_| Self::processor_error("OTLP span processor flush response is closed"))?
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        if self.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Ok(());
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        if self.control_sender.send(ProcessorControl::Shutdown(sender, timeout)).is_err() {
            self.state.store(2, Ordering::Release);
            self.abort_worker();
            return Err(Self::processor_error("OTLP span processor queue is closed"));
        }

        let result = receiver.recv_timeout(timeout).map_err(|error| {
            Self::processor_error(format!("OTLP span processor shutdown timed out: {error}"))
        })?;
        self.state.store(2, Ordering::Release);
        if result.is_err() {
            self.abort_worker();
        } else if let Ok(mut worker) = self.worker.lock() {
            worker.take();
        }
        result
    }

    fn set_resource(&mut self, resource: &Resource) {
        let _ = self.control_sender.send(ProcessorControl::SetResource(resource.clone()));
    }
}

async fn run_batch_processor<E>(
    mut span_receiver: tokio::sync::mpsc::Receiver<SpanData>,
    mut control_receiver: tokio::sync::mpsc::UnboundedReceiver<ProcessorControl>,
    exporter: Arc<tokio::sync::Mutex<E>>,
    max_export_batch_size: usize,
    batch_timeout: Duration,
) where
    E: SdkSpanExporter + 'static,
{
    let mut batch = Vec::with_capacity(max_export_batch_size);
    let mut timer = Box::pin(tokio::time::sleep(batch_timeout));

    loop {
        tokio::select! {
            biased;
            Some(control) = control_receiver.recv() => {
                match control {
                    ProcessorControl::Flush(response) => {
                        let result = match export_pending(
                            &mut span_receiver,
                            &mut batch,
                            &exporter,
                            max_export_batch_size,
                        )
                        .await {
                            Ok(()) => force_flush_exporter(Arc::clone(&exporter)).await,
                            Err(error) => Err(error),
                        };
                        let _ = response.send(result);
                    }
                    ProcessorControl::Shutdown(response, timeout) => {
                        let result = match export_pending(
                            &mut span_receiver,
                            &mut batch,
                            &exporter,
                            max_export_batch_size,
                        ).await {
                            Ok(()) => shutdown_exporter(Arc::clone(&exporter), timeout).await,
                            Err(error) => Err(error),
                        };
                        let _ = response.send(result);
                        return;
                    }
                    ProcessorControl::SetResource(resource) => {
                        let mut exporter = exporter.lock().await;
                        exporter.set_resource(&resource);
                    }
                }
            }
            Some(span) = span_receiver.recv() => {
                batch.push(span);
                if batch.len() >= max_export_batch_size {
                    let _ = export_current(&mut batch, &exporter).await;
                    timer.as_mut().reset(tokio::time::Instant::now() + batch_timeout);
                }
            }
            _ = &mut timer => {
                let _ = export_current(&mut batch, &exporter).await;
                timer.as_mut().reset(tokio::time::Instant::now() + batch_timeout);
            }
            else => return,
        }
    }
}

async fn export_pending<E>(
    span_receiver: &mut tokio::sync::mpsc::Receiver<SpanData>,
    batch: &mut Vec<SpanData>,
    exporter: &Arc<tokio::sync::Mutex<E>>,
    max_export_batch_size: usize,
) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    loop {
        while batch.len() < max_export_batch_size {
            match span_receiver.try_recv() {
                Ok(span) => batch.push(span),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        if batch.is_empty() {
            return Ok(());
        }
        export_current(batch, exporter).await?;
    }
}

async fn export_current<E>(
    batch: &mut Vec<SpanData>,
    exporter: &Arc<tokio::sync::Mutex<E>>,
) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    if batch.is_empty() {
        return Ok(());
    }
    let spans = std::mem::take(batch);
    let exporter = exporter.lock().await;
    exporter.export(spans).await
}

async fn force_flush_exporter<E>(exporter: Arc<tokio::sync::Mutex<E>>) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    match tokio::task::spawn_blocking(move || exporter.blocking_lock().force_flush()).await {
        Ok(result) => result,
        Err(error) => {
            Err(OTelSdkError::InternalFailure(format!("OTLP exporter flush task failed: {error}")))
        }
    }
}

async fn shutdown_exporter<E>(
    exporter: Arc<tokio::sync::Mutex<E>>,
    timeout: Duration,
) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    match tokio::task::spawn_blocking(move || {
        exporter.blocking_lock().shutdown_with_timeout(timeout)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(OTelSdkError::InternalFailure(format!(
            "OTLP exporter shutdown task failed: {error}"
        ))),
    }
}

/// tracing subscriber 初始化错误。
#[derive(Debug, Error)]
#[error("failed to initialize tracing subscriber: {message}")]
pub struct InitError {
    message: String,
}

/// tracing exporter 关闭错误。
#[derive(Debug, Error)]
#[error("failed to shut down tracing exporter: {message}")]
pub struct ShutdownError {
    message: String,
}

/// 进程级 tracing 运行时句柄。
///
/// 句柄持有本进程的 provider，不注册 OpenTelemetry 全局 provider。这样 Admin、Public 和
/// Worker 的装配生命周期仍由各自二进制拥有，关闭时可以明确等待有界的批次 flush。
pub struct TracingRuntime {
    provider: Option<SdkTracerProvider>,
    health: Arc<TracingHealthState>,
    shutdown_timeout: Duration,
}

impl TracingRuntime {
    /// 返回 tracing 是否建立了 OTLP exporter。
    pub fn enabled(&self) -> bool {
        self.provider.is_some()
    }

    /// 返回可注入心跳发布器的 tracing 安全状态读取器。
    pub fn health_reader(&self) -> Arc<dyn TracingHealthReader> {
        Arc::clone(&self.health) as Arc<dyn TracingHealthReader>
    }

    /// 在进程退出前关闭 exporter，并等待不超过配置的时长。
    ///
    /// 该方法会阻塞当前调用线程；异步装配根应通过 `spawn_blocking` 调用，避免阻塞 Tokio
    /// worker。重复关闭由 OpenTelemetry SDK 自身安全处理。
    pub fn shutdown(self) -> Result<(), ShutdownError> {
        let Some(provider) = self.provider else {
            return Ok(());
        };

        provider
            .shutdown_with_timeout(self.shutdown_timeout)
            .map_err(|error| ShutdownError { message: error.to_string() })
    }
}

/// 初始化 JSON tracing subscriber，不启用 OTLP exporter。
pub fn init(service: &str, configured_filter: Option<&str>) -> Result<(), InitError> {
    init_with_config(service, "unknown", "unknown", configured_filter, None, None).map(|_| ())
}

/// 初始化 JSON tracing subscriber，并按运行配置挂载一个有界 OTLP/HTTP span exporter。
pub fn init_with_config(
    service: &str,
    process_role: &str,
    version: &str,
    configured_filter: Option<&str>,
    configured: Option<&TracingConfig>,
    headers: Option<&str>,
) -> Result<TracingRuntime, InitError> {
    let filter = match configured_filter {
        Some(value) => {
            EnvFilter::try_new(value).map_err(|error| InitError { message: error.to_string() })?
        }
        None => EnvFilter::new(format!("info,{service}=debug")),
    };

    let Some(config) = configured.filter(|config| config.enabled) else {
        if headers.is_some_and(|value| !value.is_empty()) {
            return Err(InitError {
                message: "disabled tracing cannot use OTLP headers".to_owned(),
            });
        }
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json());
        subscriber.try_init().map_err(|error| InitError { message: error.to_string() })?;
        return Ok(TracingRuntime {
            provider: None,
            health: Arc::new(TracingHealthState::new(false)),
            shutdown_timeout: Duration::ZERO,
        });
    };

    let health = Arc::new(TracingHealthState::new(true));
    let provider =
        build_provider(service, process_role, version, Arc::clone(&health), config, headers)?;
    let tracer = provider.tracer(service.to_owned());
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .with(telemetry_layer);
    subscriber.try_init().map_err(|error| InitError { message: error.to_string() })?;

    Ok(TracingRuntime {
        provider: Some(provider),
        health,
        shutdown_timeout: config.shutdown_timeout,
    })
}

fn parse_headers(raw: Option<&str>) -> Result<HashMap<String, String>, InitError> {
    let raw = raw.unwrap_or_default();
    if raw.is_empty() {
        return Ok(HashMap::new());
    }
    if raw.len() > 8 * 1024 || raw.split(',').count() > 32 || raw.chars().any(char::is_control) {
        return Err(InitError { message: "OTLP headers are invalid".to_owned() });
    }
    let mut headers = HashMap::new();
    let mut seen = BTreeSet::new();
    for part in raw.split(',') {
        let Some((raw_key, raw_value)) = part.split_once('=') else {
            return Err(InitError { message: "OTLP headers are invalid".to_owned() });
        };
        let Some(key) = percent_decode(raw_key.trim()) else {
            return Err(InitError { message: "OTLP headers are invalid".to_owned() });
        };
        let Some(value) = percent_decode(raw_value.trim()) else {
            return Err(InitError { message: "OTLP headers are invalid".to_owned() });
        };
        if !valid_header_token(&key)
            || value.is_empty()
            || value.len() > 1024
            || value.chars().any(char::is_control)
        {
            return Err(InitError { message: "OTLP headers are invalid".to_owned() });
        }
        let canonical = key.to_ascii_lowercase();
        if matches!(
            canonical.as_str(),
            "baggage"
                | "connection"
                | "content-encoding"
                | "content-length"
                | "content-type"
                | "host"
                | "traceparent"
                | "tracestate"
                | "transfer-encoding"
        ) || !seen.insert(canonical)
        {
            return Err(InitError { message: "OTLP headers are invalid".to_owned() });
        }
        headers.insert(key, value);
    }
    Ok(headers)
}

fn valid_header_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex_value)?;
            let low = bytes.get(index + 2).copied().and_then(hex_value)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn build_provider(
    service: &str,
    process_role: &str,
    version: &str,
    health: Arc<TracingHealthState>,
    config: &TracingConfig,
    raw_headers: Option<&str>,
) -> Result<SdkTracerProvider, InitError> {
    if config.endpoint.is_empty()
        || !config.endpoint.split_once("://").is_some_and(|(scheme, authority)| {
            matches!(scheme, "http" | "https") && !authority.is_empty()
        })
        || !config.sample_ratio.is_finite()
        || !(0.0..=1.0).contains(&config.sample_ratio)
        || config.queue_capacity == 0
        || config.max_export_batch_size == 0
        || config.max_export_batch_size > config.queue_capacity
    {
        return Err(InitError { message: "tracing exporter configuration is invalid".to_owned() });
    }

    // reqwest 的 rustls transport 需要进程先选择一个 CryptoProvider；失败时保留已安装的
    // provider，避免多个独立装配根在同一进程内重复安装导致启动顺序依赖。
    let _ = rustls::crypto::ring::default_provider().install_default();
    let headers = parse_headers(raw_headers)?;
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(config.endpoint.clone())
        .with_timeout(config.export_timeout)
        .with_headers(headers)
        .build()
        .map_err(|_| InitError { message: "failed to build OTLP HTTP exporter".to_owned() })?;
    let processor_health = Arc::clone(&health);
    let exporter = HealthSpanExporter { exporter, health };
    let processor = DropAwareBatchSpanProcessor::new(
        exporter,
        processor_health,
        config.queue_capacity,
        config.max_export_batch_size,
        config.batch_timeout,
    )?;
    let sampler = Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.sample_ratio)));
    let resource = Resource::builder()
        .with_service_name(service.to_owned())
        .with_attributes([
            KeyValue::new("service.version", version.to_owned()),
            KeyValue::new("process.role", process_role.to_owned()),
        ])
        .build();

    Ok(SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_sampler(sampler)
        .with_resource(resource)
        .build())
}
