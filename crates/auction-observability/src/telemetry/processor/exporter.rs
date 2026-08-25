//! OTLP exporter 的安全健康结果包装。

use std::{future::Future, sync::Arc, time::Duration};

use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{SpanData, SpanExporter as SdkSpanExporter},
};

use super::super::health::{FailureCode, TracingHealthState};

/// 在不改变 OTLP exporter 行为的前提下记录安全的成功/失败结果。
#[derive(Debug)]
pub(crate) struct HealthSpanExporter<E> {
    pub(crate) exporter: E,
    pub(crate) health: Arc<TracingHealthState>,
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
