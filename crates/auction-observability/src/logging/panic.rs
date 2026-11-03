//! 原生 panic 的安全输出；替换默认 hook，不读取格式化 payload，也不改变展开或退出。

use std::{fmt, io::Write, panic::PanicHookInfo, sync::Once};

use tracing::{Event, Subscriber, subscriber::NoSubscriber};
use tracing_subscriber::{
    fmt::{
        FmtContext,
        format::{FormatEvent, FormatFields, Writer},
    },
    registry::LookupSpan,
};

const TARGET: &str = "auction::panic";
const REDACTED_PAYLOAD: &str = "<non-static panic payload redacted>";
static INSTALL: Once = Once::new();

/// panic 事件不附带当前 span；其余 JSON 日志沿用原格式。
pub(crate) struct PanicSafeJson;

impl<S, N> FormatEvent<S, N> for PanicSafeJson
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // JSON formatter 会回退到当前 span；仅 parent: None 不能排除上下文字段。
        let with_spans = event.metadata().target() != TARGET;
        let format = tracing_subscriber::fmt::format()
            .json()
            .with_current_span(with_spans)
            .with_span_list(with_spans);
        format.format_event(context, writer, event)
    }
}

/// 日志初始化后由装配根安装一次；仅保留静态字符串、线程名和源码位置。
/// 不调用旧 hook，避免默认 hook 再次输出秘密；panic 后仍由原运行时展开或退出。
pub fn install_safe_panic_hook() {
    INSTALL.call_once(|| std::panic::set_hook(Box::new(record)));
}

fn record(info: &PanicHookInfo<'_>) {
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("<unnamed>");
    let location = info.location();
    let file = location.map_or("<unknown>", |location| location.file());
    let line = location.map_or(0, |location| location.line());
    let column = location.map_or(0, |location| location.column());
    // String 和其他任意 payload 都不格式化；只接受编译期静态字符串类别。
    let payload = info.payload().downcast_ref::<&'static str>().copied();
    let payload = payload.unwrap_or(REDACTED_PAYLOAD);
    let subscriber_ready =
        tracing::dispatcher::get_default(|dispatcher| !dispatcher.is::<NoSubscriber>());
    if subscriber_ready {
        tracing::error!(
            target: TARGET,
            parent: None,
            thread_name,
            file,
            line,
            column,
            payload,
            "原生任务 panic"
        );
    } else {
        // stderr 写入失败不能在 hook 中触发第二次 panic；只写同一组安全字段。
        let _ = writeln!(
            std::io::stderr().lock(),
            "{TARGET} thread_name={thread_name:?} file={file:?} \
             line={line} column={column} payload={payload:?}"
        );
    }
}
