//! 一次性本机初始化接口；不会复用常规 Admin 会话或 CSRF API。

use auction_contracts::{SetupErrorResponse, SetupRequest, SetupResponse};
use gloo_net::http::Request;

use super::transport::AdminResponse;

/// 配置表单开放前先独立验证部署码；不发送配置或使用普通 Admin 会话。
pub(crate) async fn verify_first_run(token: &str) -> Result<bool, String> {
    let response = super::send_request(|abort_signal| {
        Request::post("/api/v1/setup/verify")
            .abort_signal(abort_signal)
            .header("x-setup-token", token)
            .build()
            .map_err(|_| "invalid_input".to_owned())
    })
    .await?;
    setup_response(response, "verified").await.map(|body| body.initialize_and_start)
}

/// 令牌仅在本次请求 header 发送，成功响应只消费稳定阶段值。
pub(crate) async fn submit_first_run(
    token: &str,
    form: &SetupRequest,
    complete: bool,
    confirmed_target: Option<(&str, u16, &str)>,
) -> Result<(), String> {
    let path = if complete { "/api/v1/setup/complete" } else { "/api/v1/setup/validate" };
    let confirmation = confirmed_target.map(|(host, port, name)| encode_target(host, port, name));
    let result = async {
        let response = super::send_request(|abort_signal| {
            let mut request =
                Request::post(path).abort_signal(abort_signal).header("x-setup-token", token);
            if let Some(confirmation) = &confirmation {
                request = request.header("x-setup-confirm-target", confirmation);
            }
            request.json(form).map_err(|_| "invalid_input".to_owned())
        })
        .await?;
        let expected = if complete { "completed" } else { "validated" };
        setup_response(response, expected).await.map(|_| ())
    }
    .await;
    // 超时或响应丢失不代表服务端未发布目录；保存动作不能提示操作者直接重试。
    result.map_err(|code| {
        if complete
            && matches!(
                code.as_str(),
                "network_error" | "request_timeout" | "invalid_response" | "setup_unknown"
            )
        {
            "setup_outcome_unknown".to_owned()
        } else {
            code
        }
    })
}

fn encode_target(host: &str, port: u16, name: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("host", host)
        .append_pair("port", &port.to_string())
        .append_pair("name", name)
        .finish()
}

async fn setup_response(response: AdminResponse, expected: &str) -> Result<SetupResponse, String> {
    if !response.ok() {
        return Err(response
            .json::<SetupErrorResponse>()
            .await
            .map_or_else(|_| "setup_unknown".to_owned(), |body| body.code));
    }
    let body: SetupResponse = response.json().await.map_err(|_| "setup_unknown".to_owned())?;
    if body.status == expected { Ok(body) } else { Err("setup_unknown".to_owned()) }
}

#[cfg(test)]
mod tests {
    use super::encode_target;

    #[test]
    fn setup_database_confirmation_has_the_canonical_header_format() {
        assert_eq!(
            encode_target("::1", 5432, "拍卖 & %"),
            "host=%3A%3A1&port=5432&name=%E6%8B%8D%E5%8D%96+%26+%25"
        );
    }
    #[test]
    fn setup_validation_contract_fixture() {
        use super::{SetupErrorResponse, SetupResponse};
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let error: SetupErrorResponse =
            decode(admin_fixture!("setup-validate-invalid"), "", "code");
        assert_eq!(error.code, "invalid_input");

        for (body, initialize) in [
            (admin_fixture!("setup-verify"), false),
            (admin_fixture!("setup-verify-initialize"), true),
        ] {
            let decoded: SetupResponse = decode(body, "", "status");
            assert_eq!(decoded.status, "verified");
            assert_eq!(decoded.initialize_and_start, initialize);
        }
    }
}
