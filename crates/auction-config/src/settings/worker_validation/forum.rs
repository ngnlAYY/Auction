//! 论坛同步 Worker 配置的边界校验。

use super::super::{ConfigError, ForumSyncWorkerConfig, Url};

pub(crate) fn validate_forum_worker(config: &ForumSyncWorkerConfig) -> Result<(), ConfigError> {
    if !matches!(config.credential_world.as_str(), "main" | "isekai" | "shared") {
        return Err(ConfigError::InvalidForumConfig(
            "credential_world 只能是 main、isekai 或 shared",
        ));
    }
    if !(1..=10_000_000).contains(&config.main_board_id)
        || !(1..=10_000_000).contains(&config.isekai_board_id)
    {
        return Err(ConfigError::InvalidForumConfig(
            "main_board_id/isekai_board_id 必须是 1 至 10000000 的板块 ID",
        ));
    }
    if config.batch_size == 0 || config.batch_size > 100 {
        return Err(ConfigError::InvalidForumConfig("batch_size 必须位于 1 至 100"));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidForumConfig("poll_interval_seconds 必须位于 1 至 86400"));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidForumConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.maximum_response_bytes == 0 || config.maximum_response_bytes > 8 * 1024 * 1024 {
        return Err(ConfigError::InvalidForumConfig("maximum_response_bytes 必须位于 1 至 8 MiB"));
    }
    if !(1..=20).contains(&config.maximum_topic_pages) {
        return Err(ConfigError::InvalidForumConfig("maximum_topic_pages 必须位于 1 至 20"));
    }
    if config.minimum_request_interval_millis == 0 {
        return Err(ConfigError::InvalidForumConfig("minimum_request_interval_millis 必须大于零"));
    }
    if !(1..=300).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidForumConfig("request_timeout_seconds 必须位于 1 至 300"));
    }
    if config.item_timeout_seconds == 0 || config.item_timeout_seconds > 3_600 {
        return Err(ConfigError::InvalidForumConfig("item_timeout_seconds 必须位于 1 至 3600"));
    }
    // 单目录页的最低预算：四路主题读取，目录/日志各一次 GET + POST。
    // 多目录页及正文校验会增加请求，部署须提高 item/lease；不能用最低预算宣称覆盖任意页数。
    let topic_pages = u64::from(config.maximum_topic_pages);
    let Some(topic_read_waves) = topic_pages.checked_add(3).map(|value| value / 4) else {
        return Err(ConfigError::InvalidForumConfig("论坛分页波次数超出可表示范围"));
    };
    let Some(request_timeout_waves) = topic_read_waves.checked_add(4) else {
        return Err(ConfigError::InvalidForumConfig("论坛请求超时波次数超出可表示范围"));
    };
    let Some(request_starts) = topic_pages.checked_add(4) else {
        return Err(ConfigError::InvalidForumConfig("论坛请求起始次数超出可表示范围"));
    };
    let Some(request_budget_millis) = config
        .request_timeout_seconds
        .checked_mul(request_timeout_waves)
        .and_then(|value| value.checked_mul(1_000))
        .and_then(|value| {
            config
                .minimum_request_interval_millis
                .checked_mul(request_starts)
                .and_then(|interval| value.checked_add(interval))
        })
        .and_then(|value| value.checked_add(1_000))
    else {
        return Err(ConfigError::InvalidForumConfig("论坛请求预算超出可表示范围"));
    };
    let Some(item_budget_millis) = config.item_timeout_seconds.checked_mul(1_000) else {
        return Err(ConfigError::InvalidForumConfig("论坛任务超时超出可表示范围"));
    };
    if item_budget_millis < request_budget_millis {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须覆盖分页读取、固定楼层写入、最小请求间隔和一秒收口预算",
        ));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidForumConfig("lease_duration_seconds 必须位于 1 至 86400"));
    }
    if config.item_timeout_seconds >= config.lease_duration_seconds {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须小于 lease_duration_seconds",
        ));
    }
    if let Some(endpoint) = &config.endpoint {
        let parsed = Url::parse(endpoint)
            .map_err(|_| ConfigError::InvalidForumConfig("endpoint 不是有效 URL"))?;
        let path_is_root = parsed.path().is_empty() || parsed.path() == "/";
        let loopback_http = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(parsed.scheme() == "https" || loopback_http)
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !path_is_root
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ConfigError::InvalidForumConfig(
                "endpoint 必须是无凭据的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    if let Some(public_base_url) = &config.public_base_url {
        let parsed = Url::parse(public_base_url)
            .map_err(|_| ConfigError::InvalidForumConfig("public_base_url 不是有效 URL"))?;
        let path_is_root = parsed.path().is_empty() || parsed.path() == "/";
        if !(parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || !path_is_root
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || public_base_url.trim() != public_base_url
        {
            return Err(ConfigError::InvalidForumConfig(
                "public_base_url 必须是无凭据、无路径和无查询的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    Ok(())
}
