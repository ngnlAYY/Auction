//! Admin 登录外壳 readiness HTTP 客户端。
//! 静态 Status 路由不发请求；登录外壳继续读取 `/health/ready`。其 503 是合法的未就绪结果，
//! 不能当成网络解析错误。

use serde::Deserialize;

use super::{invalid_response_error, send_get};

/// 单个健康探针在管理端的有限状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminHealthProbeState {
    Healthy,
    NotReady,
    Unavailable,
}

/// 管理端全局 readiness 状态，对齐旧版登录壳层的可访问性提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminReadinessState {
    Checking,
    Ready,
    Degraded,
}

/// 读取管理端全局 readiness；网络错误交给页面映射为 degraded，不向浏览器暴露下游细节。
pub(crate) async fn fetch_admin_readiness() -> Result<bool, String> {
    match fetch_ready().await? {
        AdminHealthProbeState::Healthy => Ok(true),
        AdminHealthProbeState::NotReady | AdminHealthProbeState::Unavailable => Ok(false),
    }
}

async fn fetch_ready() -> Result<AdminHealthProbeState, String> {
    let response = send_get("/health/ready").await?;
    let body = response.text().await.map_err(invalid_response_error)?;
    decode_readiness(&body, response.ok())
}

// readiness 是固定探针契约，两级对象都使用闭集字段。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadinessResponse {
    #[serde(rename = "code")]
    _code: String,
    #[serde(rename = "message")]
    _message: String,
    data: ReadinessData,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadinessData {
    ready: bool,
}

fn decode_readiness(body: &str, http_ok: bool) -> Result<AdminHealthProbeState, String> {
    let payload: ReadinessResponse = serde_json::from_str(body).map_err(invalid_response_error)?;
    Ok(match (http_ok, payload.data.ready) {
        (true, true) => AdminHealthProbeState::Healthy,
        (false, false) => AdminHealthProbeState::NotReady,
        _ => AdminHealthProbeState::Unavailable,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{AdminHealthProbeState, decode_readiness};
    use crate::api::contract_fixtures::admin_fixture;

    #[test]
    fn admin_readiness_contract_fixtures() {
        for (body, http_ok, expected) in [
            (admin_fixture!("readiness"), true, AdminHealthProbeState::Healthy),
            (admin_fixture!("readiness-unavailable"), false, AdminHealthProbeState::NotReady),
        ] {
            assert_eq!(decode_readiness(body, http_ok).expect("就绪样本"), expected);

            let mut invalid: Value = serde_json::from_str(body).expect("就绪样本");
            invalid["data"].as_object_mut().unwrap().remove("ready").expect("就绪布尔值");
            assert!(decode_readiness(&invalid.to_string(), http_ok).is_err());
        }
    }

    #[test]
    fn readiness_rejects_unknown_fields() {
        for (body, http_ok) in
            [(admin_fixture!("readiness"), true), (admin_fixture!("readiness-unavailable"), false)]
        {
            for path in ["", "/data"] {
                let mut invalid: Value = serde_json::from_str(body).expect("就绪样本");
                invalid
                    .pointer_mut(path)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .insert("extra".to_owned(), json!(true));
                assert!(decode_readiness(&invalid.to_string(), http_ok).is_err());
            }
        }
    }
}
