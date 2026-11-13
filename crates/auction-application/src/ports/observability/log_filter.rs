//! `log_filter` 的语法：逗号分隔的 `级别` 或 `target=级别`。
//! 配置预检与 tracing 初始化共用这一份解析，两处接受的写法不会漂移。
//! 不支持按 span 或字段过滤；无法识别的片段失败关闭，不退化成“某个 target 的全量日志”。

use thiserror::Error;
use tracing::level_filters::LevelFilter;

/// 一条过滤指令；`target` 为空时设置默认级别，否则按 target 前缀匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFilterDirective {
    pub target: Option<String>,
    pub level: LevelFilter,
}

/// 过滤表达式无效；不回显配置原文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("log filter is invalid")]
pub struct LogFilterError;

/// 按书写顺序返回全部指令；同一 target 重复出现时由消费方以后者为准。
/// 允许多余的逗号，但至少要有一条指令。
pub fn parse_log_filter(value: &str) -> Result<Vec<LogFilterDirective>, LogFilterError> {
    let directives = value
        .split(',')
        .filter(|directive| !directive.is_empty())
        .map(parse_directive)
        .collect::<Result<Vec<_>, _>>()?;
    if directives.is_empty() {
        return Err(LogFilterError);
    }
    Ok(directives)
}

fn parse_directive(directive: &str) -> Result<LogFilterDirective, LogFilterError> {
    let Some((target, level)) = directive.split_once('=') else {
        // 没有 `=` 的片段只能是级别；裸 target 不再等同于该 target 的 TRACE。
        return Ok(LogFilterDirective { target: None, level: parse_level(directive)? });
    };
    if !valid_target(target) {
        return Err(LogFilterError);
    }
    Ok(LogFilterDirective { target: Some(target.to_owned()), level: parse_level(level)? })
}

/// 级别名不区分大小写，也接受 0（off）到 5（trace）的数字。
fn parse_level(level: &str) -> Result<LevelFilter, LogFilterError> {
    // `LevelFilter` 把空串当作 ERROR、把 `+3` 当作数字；这里要求显式且只含字母数字。
    if level.is_empty() || !level.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(LogFilterError);
    }
    let Ok(level) = level.parse::<LevelFilter>() else {
        return Err(LogFilterError);
    };
    Ok(level)
}

/// target 是模块路径或服务名：字母、数字与 `_`、`:`、`-`、`.`。
/// 方括号、花括号与空白属于 span/字段过滤语法，一律拒绝。
fn valid_target(target: &str) -> bool {
    !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use tracing::level_filters::LevelFilter;

    use super::{LogFilterDirective, LogFilterError, parse_log_filter};

    fn directive(target: Option<&str>, level: LevelFilter) -> LogFilterDirective {
        LogFilterDirective { target: target.map(str::to_owned), level }
    }

    #[test]
    fn levels_and_target_levels_parse_in_written_order() {
        assert_eq!(parse_log_filter("info"), Ok(vec![directive(None, LevelFilter::INFO)]));
        assert_eq!(
            parse_log_filter("info,auction=debug,auction_observability::log_dedup=off"),
            Ok(vec![
                directive(None, LevelFilter::INFO),
                directive(Some("auction"), LevelFilter::DEBUG),
                directive(Some("auction_observability::log_dedup"), LevelFilter::OFF),
            ])
        );
        // 服务名带连字符；级别不区分大小写，数字 0–5 对应 off 到 trace。
        assert_eq!(
            parse_log_filter("WARN,auction-public-api=Trace,h2.codec=1,sqlx=0,hyper=5"),
            Ok(vec![
                directive(None, LevelFilter::WARN),
                directive(Some("auction-public-api"), LevelFilter::TRACE),
                directive(Some("h2.codec"), LevelFilter::ERROR),
                directive(Some("sqlx"), LevelFilter::OFF),
                directive(Some("hyper"), LevelFilter::TRACE),
            ])
        );
        // 追加片段时多出的逗号不改变含义。
        assert_eq!(
            parse_log_filter(",info,,auction::storage::audit_health=warn,"),
            Ok(vec![
                directive(None, LevelFilter::INFO),
                directive(Some("auction::storage::audit_health"), LevelFilter::WARN),
            ])
        );
    }

    #[test]
    fn span_field_and_ambiguous_forms_fail_closed() {
        for value in [
            "",
            ",",
            "inf",
            "auction",
            "auction=",
            "=debug",
            "auction=verbose",
            "auction=6",
            "auction=+3",
            "auction=debug=info",
            "info, auction=debug",
            " info",
            "info ",
            "a b=info",
            "[request]=debug",
            "auction[request]=debug",
            "auction[request{route=/x}]=debug",
            "auction[{route}]=debug",
            "[{request_id}]=trace",
            "auction{route}=debug",
            "auction=debug\n",
            "拍卖=debug",
        ] {
            assert_eq!(parse_log_filter(value), Err(LogFilterError), "{value:?}");
        }
    }
}
