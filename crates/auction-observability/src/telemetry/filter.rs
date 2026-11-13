//! 把 `log_filter` 表达式装配成进程级的 target 级别过滤器。

use auction_application::log_filter::{LogFilterDirective, parse_log_filter};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;

use super::config::InitError;

/// 配置预检已用同一语法校验；这里再次解析并失败关闭，错误不回显配置原文。
/// 指令按最长 target 前缀匹配，同一 target 重复出现时后者生效；没有默认级别时其余 target 全部关闭。
pub(super) fn target_filter(expression: &str) -> Result<Targets, InitError> {
    let directives =
        parse_log_filter(expression).map_err(|error| InitError { message: error.to_string() })?;
    Ok(directives.into_iter().fold(Targets::new(), |targets, directive| {
        let LogFilterDirective { target, level } = directive;
        match target {
            Some(target) => targets.with_target(target, level),
            None => targets.with_default(level),
        }
    }))
}

/// 未配置 `log_filter` 时的默认值，等价于 `info,<服务名>=debug`。
pub(super) fn default_filter(service: &str) -> Targets {
    Targets::new().with_default(LevelFilter::INFO).with_target(service, LevelFilter::DEBUG)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::{
        Layer,
        layer::{Context, SubscriberExt},
    };

    use super::{default_filter, target_filter};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<(&'static str, Level)>>>);

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            let metadata = event.metadata();
            self.0.lock().unwrap().push((metadata.target(), *metadata.level()));
        }
    }

    fn emit_samples() {
        tracing::error!(target: "other", "sample");
        tracing::warn!(target: "other", "sample");
        tracing::info!(target: "other", "sample");
        tracing::debug!(target: "other", "sample");
        tracing::info!(target: "auction", "sample");
        tracing::debug!(target: "auction", "sample");
        tracing::trace!(target: "auction", "sample");
        tracing::warn!(target: "auction::storage::audit_health", "sample");
        tracing::debug!(target: "auction::storage::audit_health", "sample");
        tracing::warn!(target: "auction_observability::log_dedup", "sample");
        tracing::info!(target: "auction_observability::log_dedup", "sample");
        tracing::debug!(target: "auction-public-api", "sample");
        tracing::error!(target: "auction::panic", "sample");
        tracing::trace!(target: "sqlx::query", "sample");
    }

    fn passed(
        filter: impl Layer<tracing_subscriber::Registry> + Send + Sync + 'static,
    ) -> Vec<(&'static str, Level)> {
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(filter).with(capture.clone());
        tracing::subscriber::with_default(subscriber, emit_samples);
        capture.0.lock().unwrap().clone()
    }

    const INFO_BASELINE: [(&str, Level); 8] = [
        ("other", Level::ERROR),
        ("other", Level::WARN),
        ("other", Level::INFO),
        ("auction", Level::INFO),
        ("auction::storage::audit_health", Level::WARN),
        ("auction_observability::log_dedup", Level::WARN),
        ("auction_observability::log_dedup", Level::INFO),
        ("auction::panic", Level::ERROR),
    ];

    #[test]
    fn default_level_applies_to_every_target_without_a_more_specific_directive() {
        assert_eq!(passed(target_filter("info").unwrap()), INFO_BASELINE);
        assert_eq!(
            passed(target_filter("off,auction::panic=error").unwrap()),
            [("auction::panic", Level::ERROR)]
        );
    }

    #[test]
    fn longest_target_prefix_wins_and_later_duplicates_replace_earlier_ones() {
        // `auction` 是文本前缀：同时覆盖 `auction::…`、`auction_observability::…` 与 `auction-public-api`。
        assert_eq!(
            passed(
                target_filter("info,auction=debug,auction_observability::log_dedup=off").unwrap()
            ),
            [
                ("other", Level::ERROR),
                ("other", Level::WARN),
                ("other", Level::INFO),
                ("auction", Level::INFO),
                ("auction", Level::DEBUG),
                ("auction::storage::audit_health", Level::WARN),
                ("auction::storage::audit_health", Level::DEBUG),
                ("auction-public-api", Level::DEBUG),
                ("auction::panic", Level::ERROR),
            ]
        );
        assert_eq!(
            passed(target_filter("error,other=debug,other=warn").unwrap()),
            [("other", Level::ERROR), ("other", Level::WARN), ("auction::panic", Level::ERROR)]
        );
    }

    #[test]
    fn targets_without_a_default_level_leave_everything_else_disabled() {
        assert_eq!(
            passed(target_filter("auction::storage=debug").unwrap()),
            [
                ("auction::storage::audit_health", Level::WARN),
                ("auction::storage::audit_health", Level::DEBUG),
            ]
        );
    }

    #[test]
    fn unconfigured_filter_defaults_to_info_with_service_debug() {
        let mut expected = INFO_BASELINE.to_vec();
        expected.insert(7, ("auction-public-api", Level::DEBUG));
        assert_eq!(passed(default_filter("auction-public-api")), expected);
    }

    #[test]
    fn span_and_field_directives_are_rejected_without_echoing_the_expression() {
        for expression in ["", "auction", "[request]=debug", "auction[{route}]=debug"] {
            let error = target_filter(expression).expect_err("expression must be rejected");
            assert_eq!(
                error.to_string(),
                "failed to initialize tracing subscriber: log filter is invalid",
                "{expression:?}"
            );
        }
    }
}
