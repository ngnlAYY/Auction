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

use super::super::{
    config::InitError,
    health::{FailureCode, TracingHealthState},
};

/// 有界 OTLP 批处理器的控制消息。
/// Span 使用独立的有界 channel，控制消息使用无界 channel，确保 flush/shutdown 不会因为
/// span 队列已满而永远无法送达。
enum ProcessorControl {
    Flush(std::sync::mpsc::Sender<OTelSdkResult>),
    Shutdown(std::sync::mpsc::Sender<OTelSdkResult>, tokio::time::Instant),
    SetResource(Resource),
}

const FORCE_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

/// 能准确记录队列满导致丢弃数量的异步 span processor。
/// OpenTelemetry SDK 的内置 processor 不公开丢弃计数；这里保留相同的有界批处理语义，但在
/// `try_send` 失败的同步路径直接增加进程安全计数，避免用 exporter 失败数冒充队列丢弃数。
pub(crate) struct DropAwareBatchSpanProcessor<E> {
    span_sender: tokio::sync::mpsc::Sender<SpanData>,
    control_sender: tokio::sync::mpsc::UnboundedSender<ProcessorControl>,
    _exporter: std::marker::PhantomData<fn() -> E>,
    health: Arc<TracingHealthState>,
    state: AtomicU8,
    control_admission: Mutex<()>,
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
        let worker_health = Arc::clone(&health);
        let worker = runtime.spawn(run_batch_processor(
            span_receiver,
            control_receiver,
            worker_exporter,
            worker_health,
            max_export_batch_size,
            batch_timeout,
        ));

        Ok(Self {
            span_sender,
            control_sender,
            _exporter: std::marker::PhantomData,
            health,
            state: AtomicU8::new(0),
            control_admission: Mutex::new(()),
            worker: Mutex::new(Some(worker)),
        })
    }

    fn processor_error(message: impl Into<String>) -> OTelSdkError {
        OTelSdkError::InternalFailure(message.into())
    }

    fn abort_worker(&self) {
        let mut worker = self.worker.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let _admission =
            self.control_admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.state.load(Ordering::Acquire) != 0 {
            self.health.dropped_span_count.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.span_sender.try_send(span).is_err() {
            self.health.dropped_span_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let _admission =
                self.control_admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.state.load(Ordering::Acquire) != 0 {
                return Err(Self::processor_error("OTLP span processor is shutting down"));
            }
            self.control_sender
                .send(ProcessorControl::Flush(sender))
                .map_err(|_| Self::processor_error("OTLP span processor queue is closed"))?;
        }
        match receiver.recv_timeout(FORCE_FLUSH_TIMEOUT) {
            Ok(result) => result,
            Err(error) => {
                self.state.store(2, Ordering::Release);
                self.abort_worker();
                self.health.record_failure(FailureCode::Transport);
                Err(Self::processor_error(format!(
                    "OTLP span processor flush timed out or closed: {error}"
                )))
            }
        }
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        let (sender, receiver) = std::sync::mpsc::channel();
        let deadline = tokio::time::Instant::now() + timeout;
        let admitted = {
            let _admission =
                self.control_admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
                return Ok(());
            }
            self.control_sender.send(ProcessorControl::Shutdown(sender, deadline)).is_ok()
        };
        if !admitted {
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
        if self.control_sender.send(ProcessorControl::SetResource(resource.clone())).is_err() {
            self.state.store(2, Ordering::Release);
            self.health.record_failure(FailureCode::Shutdown);
        }
    }
}

async fn run_batch_processor<E>(
    mut span_receiver: tokio::sync::mpsc::Receiver<SpanData>,
    mut control_receiver: tokio::sync::mpsc::UnboundedReceiver<ProcessorControl>,
    exporter: Arc<tokio::sync::Mutex<E>>,
    health: Arc<TracingHealthState>,
    max_export_batch_size: usize,
    batch_timeout: Duration,
) where
    E: SdkSpanExporter + 'static,
{
    let mut batch = Vec::with_capacity(max_export_batch_size);
    let mut timer = Box::pin(tokio::time::sleep(batch_timeout));
    let mut unreported_export_failure = false;

    loop {
        tokio::select! {
            biased;
            Some(control) = control_receiver.recv() => {
                match control {
                    ProcessorControl::Flush(response) => {
                        let mut result = match export_pending(
                            &mut span_receiver,
                            &mut batch,
                            &exporter,
                            &health,
                            max_export_batch_size,
                        )
                        .await
                        {
                            Ok(()) => {
                                let result = force_flush_exporter(&exporter).await;
                                record_export_failure(&health, &result, FailureCode::Transport);
                                result
                            }
                            Err(error) => Err(error),
                        };
                        if std::mem::take(&mut unreported_export_failure) && result.is_ok() {
                            result = Err(OTelSdkError::InternalFailure(
                                "a prior OTLP span export failed".to_owned(),
                            ));
                        }
                        let _ = response.send(result);
                    }
                    ProcessorControl::Shutdown(response, deadline) => {
                        span_receiver.close();
                        let mut result = match tokio::time::timeout_at(deadline, async {
                            match export_pending(
                                &mut span_receiver,
                                &mut batch,
                                &exporter,
                                &health,
                                max_export_batch_size,
                            )
                            .await
                            {
                                Ok(()) => {
                                    let remaining = deadline.saturating_duration_since(
                                        tokio::time::Instant::now(),
                                    );
                                    let result = shutdown_exporter(&exporter, remaining).await;
                                    record_export_failure(&health, &result, FailureCode::Shutdown);
                                    result
                                }
                                Err(error) => Err(error),
                            }
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err(OTelSdkError::InternalFailure(
                                "OTLP span processor shutdown timed out".to_owned(),
                            )),
                        };
                        if std::mem::take(&mut unreported_export_failure) && result.is_ok() {
                            result = Err(OTelSdkError::InternalFailure(
                                "a prior OTLP span export failed".to_owned(),
                            ));
                        }
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
                    if export_current(&mut batch, &exporter, &health).await.is_err() {
                        unreported_export_failure = true;
                    }
                    timer.as_mut().reset(tokio::time::Instant::now() + batch_timeout);
                }
            }
            _ = &mut timer => {
                if export_current(&mut batch, &exporter, &health).await.is_err() {
                    unreported_export_failure = true;
                }
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
    health: &TracingHealthState,
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
        export_current(batch, exporter, health).await?;
    }
}

async fn export_current<E>(
    batch: &mut Vec<SpanData>,
    exporter: &Arc<tokio::sync::Mutex<E>>,
    health: &TracingHealthState,
) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    if batch.is_empty() {
        return Ok(());
    }
    // exporter 取得整批所有权；下一批仍预留同一上限，避免每轮从零开始逐级扩容。
    let next_batch = Vec::with_capacity(batch.capacity());
    let spans = std::mem::replace(batch, next_batch);
    let exporter = exporter.lock().await;
    let result = exporter.export(spans).await;
    record_export_result(health, &result, FailureCode::Transport);
    result
}

fn record_export_result(
    health: &TracingHealthState,
    result: &OTelSdkResult,
    failure_code: FailureCode,
) {
    if result.is_ok() {
        health.record_success();
    } else {
        health.record_failure(failure_code);
    }
}

fn record_export_failure(
    health: &TracingHealthState,
    result: &OTelSdkResult,
    failure_code: FailureCode,
) {
    if result.is_err() {
        health.record_failure(failure_code);
    }
}

async fn force_flush_exporter<E>(exporter: &Arc<tokio::sync::Mutex<E>>) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    let exporter = exporter.lock().await;
    exporter.force_flush()
}

async fn shutdown_exporter<E>(
    exporter: &Arc<tokio::sync::Mutex<E>>,
    timeout: Duration,
) -> OTelSdkResult
where
    E: SdkSpanExporter + 'static,
{
    let exporter = exporter.lock().await;
    exporter.shutdown_with_timeout(timeout)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Condvar, Mutex, mpsc},
        thread::ThreadId,
        time::Duration,
    };

    use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
    use opentelemetry_sdk::{
        Resource,
        error::OTelSdkError,
        trace::{SdkTracerProvider, SpanExporter, SpanProcessor},
    };

    use super::{DropAwareBatchSpanProcessor, TracingHealthState};
    use crate::{health::TracingStatus, telemetry::health::FailureCode};

    #[derive(Debug)]
    struct ShutdownProbe {
        gate: (Mutex<bool>, Condvar),
        started: mpsc::Sender<ThreadId>,
        finished: mpsc::Sender<ThreadId>,
    }

    #[derive(Debug)]
    struct TestExporter {
        flush_ok: bool,
        export_fails: bool,
        export_result: Option<mpsc::Sender<bool>>,
        resource_thread: Option<mpsc::Sender<ThreadId>>,
        shutdown_probe: Option<Arc<ShutdownProbe>>,
    }

    impl SpanExporter for TestExporter {
        async fn export(
            &self,
            _batch: Vec<opentelemetry_sdk::trace::SpanData>,
        ) -> opentelemetry_sdk::error::OTelSdkResult {
            if let Some(sender) = &self.export_result {
                let _ = sender.send(self.export_fails);
            }
            if self.export_fails {
                Err(OTelSdkError::InternalFailure("export failure".to_owned()))
            } else {
                Ok(())
            }
        }

        fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
            if self.flush_ok {
                Ok(())
            } else {
                Err(OTelSdkError::InternalFailure("flush failure".to_owned()))
            }
        }

        fn shutdown_with_timeout(
            &self,
            _timeout: Duration,
        ) -> opentelemetry_sdk::error::OTelSdkResult {
            if let Some(probe) = &self.shutdown_probe {
                let current = std::thread::current().id();
                let _ = probe.started.send(current);
                let (released, condition) = &probe.gate;
                let mut released =
                    released.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*released {
                    released =
                        condition.wait(released).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                let _ = probe.finished.send(current);
            }
            Ok(())
        }

        fn set_resource(&mut self, _resource: &Resource) {
            if let Some(sender) = &self.resource_thread {
                let _ = sender.send(std::thread::current().id());
            }
        }
    }

    fn processor(
        exporter: TestExporter,
        health: Arc<TracingHealthState>,
    ) -> DropAwareBatchSpanProcessor<TestExporter> {
        DropAwareBatchSpanProcessor::new(exporter, health, 1, 1, Duration::from_secs(60))
            .unwrap_or_else(|error| panic!("processor creation failed: {error}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exporter_failure_is_counted_once() {
        let health = Arc::new(TracingHealthState::new(true));
        let (export_tx, export_rx) = mpsc::channel();
        let processor = processor(
            TestExporter {
                flush_ok: true,
                export_fails: true,
                export_result: Some(export_tx),
                resource_thread: None,
                shutdown_probe: None,
            },
            Arc::clone(&health),
        );
        let provider = SdkTracerProvider::builder().with_span_processor(processor).build();
        let tracer = provider.tracer("telemetry-test");
        tracer.start("failed-export").end();

        assert!(
            export_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_else(|error| panic!("export was not attempted: {error}"))
        );
        let _ = provider.force_flush();
        let snapshot = health.snapshot();
        assert_eq!(snapshot.export_failure_count, 1);
        assert_eq!(snapshot.status, TracingStatus::Degraded);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_flush_and_shutdown_preserve_degraded_health() {
        let health = Arc::new(TracingHealthState::new(true));
        health.record_failure(FailureCode::Transport);
        let processor = processor(
            TestExporter {
                flush_ok: true,
                export_fails: false,
                export_result: None,
                resource_thread: None,
                shutdown_probe: None,
            },
            Arc::clone(&health),
        );

        assert!(processor.force_flush().is_ok());
        let after_flush = health.snapshot();
        assert_eq!(after_flush.status, TracingStatus::Degraded);
        assert_eq!(after_flush.export_failure_count, 1);
        assert!(after_flush.last_success_at.is_none());

        assert!(processor.shutdown_with_timeout(Duration::from_secs(1)).is_ok());
        let after_shutdown = health.snapshot();
        assert_eq!(after_shutdown.status, TracingStatus::Degraded);
        assert_eq!(after_shutdown.export_failure_count, 1);
        assert!(after_shutdown.last_success_at.is_none());
    }

    #[test]
    fn timed_out_shutdown_remains_owned_by_batch_worker() {
        let (resource_tx, resource_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let (processor_tx, processor_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let probe = Arc::new(ShutdownProbe {
            gate: (Mutex::new(false), Condvar::new()),
            started: started_tx,
            finished: finished_tx,
        });
        let worker_probe = Arc::clone(&probe);
        let runtime_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap_or_else(|error| panic!("test runtime creation failed: {error}"));
            runtime.block_on(async move {
                let processor = processor(
                    TestExporter {
                        flush_ok: true,
                        export_fails: false,
                        export_result: None,
                        resource_thread: Some(resource_tx),
                        shutdown_probe: Some(worker_probe),
                    },
                    Arc::new(TracingHealthState::new(true)),
                );
                let _ = processor_tx.send(processor);
                let _ = stop_rx.await;
            });
        });
        let mut processor = processor_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("processor was not created: {error}"));
        processor.set_resource(&Resource::builder().build());
        let worker_thread = resource_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("batch worker did not receive resource: {error}"));

        let result = processor.shutdown_with_timeout(Duration::from_millis(50));
        assert!(result.is_err());
        let shutdown_thread = started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("shutdown was not called: {error}"));

        let (released, condition) = &probe.gate;
        *released.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_one();
        let finished_thread = finished_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("timed-out shutdown work was detached: {error}"));
        assert_eq!(shutdown_thread, worker_thread);
        assert_eq!(finished_thread, worker_thread);
        drop(processor);
        let _ = stop_tx.send(());
        runtime_thread.join().unwrap_or_else(|_| panic!("test runtime thread failed"));
    }
}
