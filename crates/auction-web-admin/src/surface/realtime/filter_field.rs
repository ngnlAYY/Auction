//! 实时与审计共用的地址筛选值、校验和请求门禁。

use std::future::Future;

use crate::api::valid_safe_text;

/// 表单草稿或已恢复的筛选。无效地址保留每个原值，直到用户明确编辑并提交。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::surface) enum FilterValue {
    Text(String),
    Invalid(Vec<String>),
}

impl Default for FilterValue {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for FilterValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl FilterValue {
    /// 仅单值可以通过校验。重复值的顺序、空值和相同值都保留。
    pub(in crate::surface) fn restore(
        pairs: &[(String, String)],
        key: &str,
        normalize: impl FnOnce(&str) -> Option<String>,
    ) -> Self {
        let mut values = pairs.iter().filter(|(name, _)| name == key).map(|(_, value)| value);
        let Some(first) = values.next() else { return Self::default() };
        if let Some(second) = values.next() {
            let mut originals = vec![first.clone(), second.clone()];
            originals.extend(values.cloned());
            return Self::Invalid(originals);
        }
        normalize(first).map_or_else(|| Self::Invalid(vec![first.clone()]), Self::Text)
    }

    /// 无效地址不能从其中择一发起请求，即使该值单独看是合法的。
    pub(in crate::surface) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            Self::Invalid(_) => None,
        }
    }

    /// 保留单个原值供编辑，其余原值在字段旁显示。
    pub(in crate::surface) fn input_value(&self) -> &str {
        match self {
            Self::Text(value) => value,
            Self::Invalid(values) => values.first().map(String::as_str).unwrap_or_default(),
        }
    }

    /// 获得要在错误区域显示的原值，提交校验失败的草稿也走相同的展示路径。
    pub(in crate::surface) fn error_values(&self, invalid_draft: bool) -> Vec<String> {
        match self {
            Self::Invalid(values) => values.clone(),
            Self::Text(value) if invalid_draft => vec![value.clone()],
            Self::Text(_) => Vec::new(),
        }
    }

    /// 其他筛选或分页写回时，保留尚未修正的无效参数。
    pub(in crate::surface) fn append_pairs(&self, pairs: &mut Vec<(String, String)>, key: &str) {
        let values = match self {
            Self::Text(value) if value.is_empty() => return,
            Self::Text(value) => std::slice::from_ref(value),
            Self::Invalid(values) => values.as_slice(),
        };
        pairs.extend(values.iter().map(|value| (key.to_owned(), value.clone())));
    }
}

/// 浏览器会净化部分控制字符，须在裁剪空白之前拒绝它们。
pub(in crate::surface) fn normalize_filter_input(input: &str) -> Option<&str> {
    (!input.chars().any(char::is_control)).then(|| input.trim())
}

/// 可选文本允许为空，非空时按服务端的 UTF-8 字节上限校验。
pub(in crate::surface) fn normalize_filter_text(input: &str, maximum: usize) -> Option<&str> {
    let value = normalize_filter_input(input)?;
    (value.is_empty() || valid_safe_text(value, maximum)).then_some(value)
}

/// 无效的已应用条件不能变成全量请求，刷新和轮询也走同一门禁。
pub(in crate::surface) async fn fetch_filtered<Q, T, F>(
    request: Option<Q>,
    fetch: impl FnOnce(Q) -> F,
) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    let request = request.ok_or_else(|| "invalid_query".to_owned())?;
    fetch(request).await
}

/// 事件及失败查询接受的筛选字段。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::surface) enum FilterField {
    EventStream,
    EventKind,
    FailureStream,
    FailureConsumer,
    FailureCode,
    FailureState,
}

impl FilterField {
    /// 空白输入表示未设筛选，非空值按应用端口的字节和字符规则校验。
    pub(in crate::surface) fn normalize(self, input: &str) -> Option<&str> {
        let maximum = match self {
            Self::EventStream | Self::FailureStream => 256,
            Self::EventKind | Self::FailureConsumer => 128,
            Self::FailureCode => 64,
            Self::FailureState => {
                let value = normalize_filter_input(input)?;
                return matches!(value, "" | "dead_letter" | "completed").then_some(value);
            }
        };
        normalize_filter_text(input, maximum)
            .filter(|value| self != Self::FailureCode || !value.chars().any(char::is_whitespace))
    }

    pub(super) const fn input_id(self) -> &'static str {
        match self {
            Self::EventStream => "realtime-event-stream",
            Self::EventKind => "realtime-event-kind",
            Self::FailureStream => "realtime-failure-stream",
            Self::FailureConsumer => "realtime-failure-consumer",
            Self::FailureCode => "realtime-failure-code",
            Self::FailureState => "outbox-status",
        }
    }
}
