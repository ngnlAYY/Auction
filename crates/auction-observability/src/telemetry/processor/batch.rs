//! 有界 OTLP span 队列、批处理和关闭控制。

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use opentelemetry::Context;
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    trace::{Span, SpanData, SpanExporter as SdkSpanExporter, SpanProcessor},
};

use super::super::{config::InitError, health::TracingHealthState};

/// 有界 OTLP 批处理器的控制消息。
/// Span 使用独立的有界 channel，控制消息使用无界 channel，确保 flush/shutdown 不会因为
/// span 队列已满而永远无法送达。
enum ProcessorControl {
    Flush(std::sync::mpsc::Sender<OTelSdkResult>),
    Shutdown(std::sync::mpsc::Sender<OTelSdkResult>, Duration),
    SetResource(Resource),
}

/// 能准确记录队列满导致丢弃数量的异步 span processor。
/// OpenTelemetry SDK 的内置 processor 不公开丢弃计数；这里保留相同的有界批处理语义，但在
/// `try_send` 失败的同步路径直接增加进程安全计数，避免用 exporter 失败数冒充队列丢弃数。
pub(crate) struct DropAwareBatchSpanProcessor<E> {
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
    pub(crate) fn new(
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
        let mut worker = self.worker.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(worker) = worker.take() {
            worker.abort();
        }
    }
}

impl<E> Drop for DropAwareBatchSpanProcessor<E> {
    fn drop(&mut self) {
        // 初始化失败或未显式 shutdown 时，不能让定时器和 exporter 脱离 processor 存活。
        if let Some(worker) =
            self.worker.get_mut().unwrap_or_else(|error| error.into_inner()).take()
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

        // 等待超时同样必须走下面的终态与 abort；提前返回会留下仍在导出的后台任务。
        let result = match receiver.recv_timeout(timeout) {
            Ok(result) => result,
            Err(error) => Err(Self::processor_error(format!(
                "OTLP span processor shutdown timed out: {error}"
            ))),
        };
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
                        span_receiver.close();
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
    // flush 只排空进入本轮时已入队的前缀；持续到来的新 span 不能无限延长控制消息。
    let mut remaining = span_receiver.len();
    loop {
        while remaining > 0 && batch.len() < max_export_batch_size {
            match span_receiver.try_recv() {
                Ok(span) => {
                    batch.push(span);
                    remaining -= 1;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    remaining = 0;
                    break;
                }
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
