//! tracing exporter 包装与批处理器装配。

mod batch;
mod exporter;

pub(super) use batch::DropAwareBatchSpanProcessor;
pub(super) use exporter::HealthSpanExporter;
