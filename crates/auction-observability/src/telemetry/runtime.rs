//! tracing subscriber、W3C context 和 OTLP provider 的进程运行时装配。

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use opentelemetry::{
    KeyValue,
    propagation::{Extractor, TextMapPropagator},
    trace::{TraceContextExt, TracerProvider as _},
};
use opentelemetry_otlp::{RetryPolicy, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracerProvider},
};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use super::{
    config::{InitError, ShutdownError, TracingConfig},
    health::TracingHealthState,
    processor::{DropAwareBatchSpanProcessor, HealthSpanExporter},
};

/// 传播器仅借用两个已筛选的头，不为每次 HTTP 请求复制键、值或建立哈希表。
struct TraceContextHeaders<'a> {
    traceparent: &'a str,
    tracestate: Option<&'a str>,
}

impl Extractor for TraceContextHeaders<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        if key.eq_ignore_ascii_case("traceparent") {
            Some(self.traceparent)
        } else if key.eq_ignore_ascii_case("tracestate") {
            self.tracestate
        } else {
            None
        }
    }

    fn keys(&self) -> Vec<&str> {
        if self.tracestate.is_some() {
            vec!["traceparent", "tracestate"]
        } else {
            vec!["traceparent"]
        }
    }
}

/// 把经过 HTTP 边界筛选的 W3C trace context 设为当前 server span 的远端父级。
/// 无效或超长字段会被忽略；函数不记录 header 原文，避免把不受信任的传播数据带入日志。
pub fn attach_remote_trace_context(
    span: &tracing::Span,
    traceparent: Option<&str>,
    tracestate: Option<&str>,
) {
    let Some(traceparent) = traceparent.filter(|value| value.len() <= 512) else {
        return;
    };
    let carrier = TraceContextHeaders {
        traceparent,
        tracestate: tracestate.filter(|value| value.len() <= 1024),
    };
    let context = TraceContextPropagator::new().extract(&carrier);
    if context.span().span_context().is_valid() {
        let _ = span.set_parent(context);
    }
}

/// 进程级 tracing 运行时句柄。
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
    pub fn health_reader(&self) -> Arc<dyn crate::health::TracingHealthReader> {
        Arc::clone(&self.health) as Arc<dyn crate::health::TracingHealthReader>
    }

    /// 在进程退出前关闭 exporter，并等待不超过配置的时长。
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

/// 安装仅写 stderr 的 JSON subscriber，不启用 OTLP，也不读取环境过滤器。
/// 调用方显式选择安全 target；事件不附加父 span 字段，避免污染结构化命令输出或夹带上下文。
pub fn init_json_stderr(configured_filter: &str) -> Result<(), InitError> {
    let filter = EnvFilter::try_new(configured_filter)
        .map_err(|error| InitError { message: error.to_string() })?;
    let subscriber = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(std::io::stderr),
    );
    subscriber.try_init().map_err(|error| InitError { message: error.to_string() })
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
        // OTel 0.33 默认启用重试；显式保留每批一次导出的预算与失败计数语义。
        .with_retry_policy(RetryPolicy::disabled())
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
