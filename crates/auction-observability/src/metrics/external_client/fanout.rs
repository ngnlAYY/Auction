//! 外部客户端事件的多路复用；Prometheus 记录器与进程内快照各自消费同一事件。

use std::sync::Arc;

use auction_application::external_metrics::{ExternalMetricsEvent, ExternalMetricsObserver};

/// 顺序广播事件；任一订阅者都必须是同步、无 I/O 的，因此广播不改变调用结果。
pub(crate) struct ExternalMetricsFanout {
    observers: Vec<Arc<dyn ExternalMetricsObserver>>,
}

impl ExternalMetricsFanout {
    pub(crate) fn new(observers: Vec<Arc<dyn ExternalMetricsObserver>>) -> Self {
        Self { observers }
    }
}

impl ExternalMetricsObserver for ExternalMetricsFanout {
    fn observe(&self, event: ExternalMetricsEvent) {
        for observer in &self.observers {
            observer.observe(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use auction_application::external_metrics::{
        ExternalMetricsEvent, ExternalMetricsObserver, MailOperation, MailWorld,
    };

    use super::ExternalMetricsFanout;

    struct Counter(AtomicUsize);

    impl ExternalMetricsObserver for Counter {
        fn observe(&self, _event: ExternalMetricsEvent) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn every_observer_receives_each_event_once() {
        let first = Arc::new(Counter(AtomicUsize::new(0)));
        let second = Arc::new(Counter(AtomicUsize::new(0)));
        let fanout = ExternalMetricsFanout::new(vec![
            Arc::clone(&first) as Arc<dyn ExternalMetricsObserver>,
            Arc::clone(&second) as Arc<dyn ExternalMetricsObserver>,
        ]);
        fanout.observe(ExternalMetricsEvent::MailRequestStarted {
            world: MailWorld::Main,
            operation: MailOperation::Inbox,
        });
        assert_eq!(first.0.load(Ordering::Relaxed), 1);
        assert_eq!(second.0.load(Ordering::Relaxed), 1);
    }
}
