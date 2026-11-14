//! 实时 WebSocket 控制帧：建连首帧的服务端构建号。
//! 与业务事件共用严格 JSON 边界，但形状独立：只接受 `{"type":"hello","build":"<构建号>"}`，
//! 任何多余、缺失或非法值都返回 `None`，由事件解码路径按未知帧处理。

use serde_json::Value;

use super::{json_keys_only, object, string};

/// 解析构建号首帧；返回 `Some(build)` 时调用方只更新过期提示，不进入事件序列。
/// 实时订阅只在 WASM 构建存在，宿主构建保留该纯函数供测试。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn decode_build_hello(message: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(message).ok()?;
    let value = object(&value)?;
    if !json_keys_only(value, &["type", "build"], &[]) || string(value.get("type")) != Some("hello")
    {
        return None;
    }
    let build = string(value.get("build"))?;
    auction_web_shared::build_id_is_valid(build).then(|| build.to_owned())
}

#[cfg(test)]
mod tests {
    use super::decode_build_hello;

    #[test]
    fn accepts_only_the_exact_build_frame() {
        assert_eq!(
            decode_build_hello(r#"{"type":"hello","build":"build_2026-11-04.abc"}"#).as_deref(),
            Some("build_2026-11-04.abc")
        );
        assert!(decode_build_hello(r#"{"build":"0.1.0","type":"hello"}"#).is_some());
    }

    #[test]
    fn rejects_unknown_shapes_and_invalid_builds() {
        for message in [
            r#"{"type":"hello"}"#,
            r#"{"type":"hello","build":"0.1.0","extra":1}"#,
            r#"{"type":"bye","build":"0.1.0"}"#,
            r#"{"type":"hello","build":""}"#,
            r#"{"type":"hello","build":"two words"}"#,
            r#"{"type":"hello","build":7}"#,
            r#"["hello"]"#,
            "not json",
        ] {
            assert!(decode_build_hello(message).is_none(), "{message}");
        }
    }

    #[test]
    fn does_not_swallow_event_frames() {
        // 事件信封不是 hello 形状，必须继续交给严格事件解码器。
        let event = r#"{"event_id":"1f0c","stream_sequence":2,"kind":"auction.opened","schema_version":1,"payload":{},"created_at":"2026-10-30T12:00:00Z"}"#;
        assert!(decode_build_hello(event).is_none());
    }
}
