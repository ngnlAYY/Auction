//! 赛季早鸟期表单字段。旧 runtime 文档可省略整个区块：未改动时不写入，保持文件与命令账本指纹；
//! 字段默认值与服务端 `AuctionEarlyBirdConfig::default()` 一致，完整规则仍由服务端校验。

use serde_json::{Map, Number, Value, json};

/// 早鸟区块在 runtime 文档中的路径。
pub(super) const ROOT: &str = "operations.schedule.early_bird";

pub(super) const TEXT_FIELDS: &[&str] = &[
    "operations.schedule.early_bird.timezone",
    "operations.schedule.early_bird.season_start_time",
    "operations.schedule.early_bird.open_time",
];

pub(super) const LIST_FIELDS: &[&str] = &[
    "operations.schedule.early_bird.worlds",
    "operations.schedule.early_bird.season_starts",
    "operations.schedule.early_bird.qualities",
];

/// 按行输入的正整数列表。
pub(super) const NUMBER_LIST_FIELDS: &[&str] = &["operations.schedule.early_bird.session_days"];

pub(super) const NUMBER_FIELDS: &[&str] = &[
    "operations.schedule.early_bird.window_days",
    "operations.schedule.early_bird.preview_lead_minutes",
    "operations.schedule.early_bird.live_minutes",
];

/// 与服务端默认值一致的完整区块。
pub(super) fn default_block() -> Value {
    json!({
        "enabled": false,
        "timezone": "UTC",
        "worlds": ["isekai"],
        "season_starts": ["05-01", "11-01"],
        "season_start_time": "09:00",
        "window_days": 7,
        "session_days": [1, 3, 5, 7],
        "open_time": "18:00",
        "preview_lead_minutes": 120,
        "live_minutes": 1440,
        "qualities": [],
        "pause_intake_before_season": true
    })
}

/// 读取文档中的早鸟区块；缺省时返回 `None`。
pub(super) fn block(document: &Value) -> Option<&Value> {
    document.pointer("/operations/schedule/early_bird")
}

/// 区块字段名，例如 `operations.schedule.early_bird.timezone` 返回 `timezone`。
pub(super) fn field(path: &str) -> Option<&str> {
    path.strip_prefix(ROOT)?.strip_prefix('.')
}

/// 把区块（缺省时为默认区块）投影为输入框文本；列表按行展示。
pub(super) fn inputs(document: &Value) -> Result<Vec<(&'static str, String)>, ()> {
    let defaults;
    let source = match block(document) {
        Some(source) => source,
        None => {
            defaults = default_block();
            &defaults
        }
    };
    let mut inputs = Vec::new();
    for path in TEXT_FIELDS {
        let value = source.get(field(path).ok_or(())?).and_then(Value::as_str).ok_or(())?;
        inputs.push((*path, value.to_owned()));
    }
    for path in LIST_FIELDS {
        let values = source.get(field(path).ok_or(())?).and_then(Value::as_array).ok_or(())?;
        let lines = values.iter().map(Value::as_str).collect::<Option<Vec<_>>>().ok_or(())?;
        inputs.push((*path, lines.join("\n")));
    }
    for path in NUMBER_LIST_FIELDS {
        let values = source.get(field(path).ok_or(())?).and_then(Value::as_array).ok_or(())?;
        let lines = values
            .iter()
            .map(|value| value.as_u64().map(|number| number.to_string()))
            .collect::<Option<Vec<_>>>()
            .ok_or(())?;
        inputs.push((*path, lines.join("\n")));
    }
    for path in NUMBER_FIELDS {
        let value = source.get(field(path).ok_or(())?).and_then(Value::as_u64).ok_or(())?;
        inputs.push((*path, value.to_string()));
    }
    Ok(inputs)
}

/// 开关字段在区块缺省时读取默认值，避免把默认开启的暂停收件显示为关闭。
pub(super) fn boolean(document: &Value, path: &str) -> Option<bool> {
    let name = field(path)?;
    match block(document) {
        Some(source) => source.get(name)?.as_bool(),
        None => match name {
            "enabled" => Some(false),
            "pause_intake_before_season" => Some(true),
            _ => None,
        },
    }
}

/// 确保文档存在早鸟区块；缺省时写入默认区块后再由调用方覆盖具体字段。
pub(super) fn ensure_block(document: &mut Value) -> Result<&mut Map<String, Value>, ()> {
    let schedule =
        document.pointer_mut("/operations/schedule").and_then(Value::as_object_mut).ok_or(())?;
    schedule.entry("early_bird").or_insert_with(default_block).as_object_mut().ok_or(())
}

/// 输入框的提交值；数字或数字列表处于非法中间态时拒绝提交。
pub(super) fn submitted_value(path: &str, input: &str) -> Result<Value, ()> {
    let lines = || input.lines().map(str::trim).filter(|line| !line.is_empty());
    if TEXT_FIELDS.contains(&path) {
        return Ok(Value::String(input.to_owned()));
    }
    if LIST_FIELDS.contains(&path) {
        return Ok(Value::Array(lines().map(|line| Value::String(line.to_owned())).collect()));
    }
    if NUMBER_LIST_FIELDS.contains(&path) {
        return lines()
            .map(|line| line.parse::<u64>().map(|number| Value::Number(Number::from(number))))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
            .map_err(|_| ());
    }
    if NUMBER_FIELDS.contains(&path) {
        return input
            .parse::<u64>()
            .map(|number| Value::Number(Number::from(number)))
            .map_err(|_| ());
    }
    Err(())
}

/// 所有可编辑输入路径。
pub(super) fn input_paths() -> impl Iterator<Item = &'static str> {
    TEXT_FIELDS.iter().chain(LIST_FIELDS).chain(NUMBER_LIST_FIELDS).chain(NUMBER_FIELDS).copied()
}
