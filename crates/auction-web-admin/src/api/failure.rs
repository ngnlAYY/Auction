//! Admin 同源错误 envelope 的解码与稳定失败码投影。
//! 服务端正文的自然语言不进入界面；页面只消费稳定 `code`，再经 Fluent 映射。HTTP 状态
//! 单独保留，供 404、409 等需改变交互流程的页面判断。

use std::borrow::Cow;

use auction_web_shared::{BrowserErrorClass, report_browser_error};
use gloo_net::http::Response;
use serde::Deserialize;

use super::transport::{AdminResponse, VerifiedWriteReceipt};

// 仅浏览器构造，供仍使用 String 的调用方区分「业务请求未发送」和发送后的传输故障。
const NOT_SENT_PREFIX: &str = "not_sent:";
// 拒绝服务端提供旧的浏览器内部标记；展示入口仍能解包遗留文案输入。
const WRITE_ENDPOINT_PREFIX: &str = "write_endpoint:";

// 端点类别与样本标签在同一处声明，新增类别时测试枚举也随之更新。
macro_rules! write_endpoints {
    ($($endpoint:ident => $label:literal),+ $(,)?) => {
        /// 实际发出请求的业务写端点类别。
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum AdminWriteEndpoint {
            $($endpoint),+
        }

        #[cfg(test)]
        impl AdminWriteEndpoint {
            /// 共用样本使用的全部端点类别及其标签。
            pub(crate) const SAMPLE_ENDPOINTS: &'static [(&'static str, Self)] =
                &[$(($label, Self::$endpoint)),+];

            fn label(self) -> &'static str {
                match self {
                    $(Self::$endpoint => $label),+
                }
            }
        }
    };
}

write_endpoints! {
    General => "general",
    UserMutation => "user-mutation",
    RenameRevert => "rename-revert",
    RulesPublish => "rules-publish",
    AboutPublish => "about-publish",
    MailTemplate => "mail-template",
}

impl AdminWriteEndpoint {
    fn parse(label: &str) -> Option<Self> {
        match label {
            "user-mutation" => Some(Self::UserMutation),
            "rename-revert" => Some(Self::RenameRevert),
            "rules-publish" => Some(Self::RulesPublish),
            "about-publish" => Some(Self::AboutPublish),
            "mail-template" => Some(Self::MailTemplate),
            _ => None,
        }
    }

    fn is_versioned_mutation(self) -> bool {
        // 这些端点的 state_conflict 只来自冻结版本的递增围栏；运行配置可已有持久化意图。
        matches!(self, Self::UserMutation | Self::RulesPublish | Self::AboutPublish)
    }
}

/// 命令失败对本次和此前未知尝试的证明范围。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    Unknown,
    ThisAttempt,
    ReplayChecked,
}

// 生产保留字符串 match，测试从同一份声明枚举回执规则。
macro_rules! write_rejections {
    ($($scope:ident $status:literal => [$($code:tt),+ $(,)?];)+) => {
        fn write_rejection_rule(code: &str) -> Option<(u16, Rejection)> {
            match code {
                $($($code)|+ => Some(($status, Rejection::$scope)),)+
                _ => None,
            }
        }

        /// 枚举生产码表供共用样本检查，不参与裁定。
        #[cfg(test)]
        pub(crate) fn write_rejection_rules() -> impl Iterator<Item = (&'static str, u16, Rejection)> {
            [$($(($code, $status, Rejection::$scope),)+)+].into_iter()
        }
    };
}

write_rejections! {
    // 同码的所有写入口都必须排除命令效果。已提交且仍可生效的持久化意图保持未知。
    ThisAttempt 400 => [
        "invalid_request_id",
        "invalid_query",
        "invalid_json",
        "invalid_replay",
        "invalid_command_id",
        "invalid_auction_id",
        "invalid_auction_command",
        "invalid_auction_schedule",
        "invalid_auction_intake",
        "unsupported_equipment_source",
        "invalid_lot_id",
        "invalid_lot_action",
        "invalid_lot_command",
        "invalid_recovery_command",
        "invalid_world",
        "invalid_world_command",
        "invalid_delivery_id",
        "invalid_user_id",
        "invalid_user_command",
        "invalid_rename_id",
        "invalid_rename_command",
        "invalid_delivery_command",
        "invalid_alert_command",
        "invalid_credential_action",
        "invalid_credential_kind",
        "invalid_credential_label",
        "invalid_credential_player_name",
        "invalid_credential_request",
        "invalid_credential_secret",
        "invalid_credential_status",
        "invalid_credential_world",
        "invalid_mail_request",
        "invalid_runtime_config",
        "invalid_locale",
        "invalid_rule_document",
        "invalid_settlement_return_command",
        "invalid_settlement_return_id",
    ];
    ThisAttempt 401 => ["unauthenticated"];
    ThisAttempt 403 => [
        "origin_forbidden",
        "csrf_invalid",
        "manual_early_settlement_disabled",
    ];
    ThisAttempt 404 => [
        "not_found",
        "mail_template_not_found",
        "world_not_found",
        "delivery_not_found",
        "settlement_return_not_found",
    ];
    ThisAttempt 405 => ["method_not_allowed"];
    ThisAttempt 409 => [
        "command_conflict",
        "credential_conflict",
        "stale_credential_state",
        "rename_name_taken",
        "user_conflict",
        "stale_state_version",
        "stale_world_state",
        "world_has_active_auction",
        "auction_intake_unavailable",
        "seller_blacklisted",
        "equipment_conflict",
        "stale_recovery_state",
        "stale_delivery_state",
        "invalid_auction_state",
        "invalid_lot_state",
        "delivery_sent_sighted",
        // 换绑可恢复，且快照校验早于兼容旧实例的第二次账本检查，两者只裁定本次。
        "delivery_recipient_renamed",
        "delivery_command_snapshot_unreadable",
        "settlement_obligation_ineligible",
        "sent_proof_rejected",
        "settlement_return_action_conflict",
        "settlement_return_resend_limit",
        "settlement_return_obligation_ineligible",
        "settlement_return_send_unresolved",
        "settlement_return_observation_required",
        "recovery_send_unattributed",
        "recovery_not_confirmable",
        "recovery_evidence_unresolved",
        "external_access_paused",
        "poll_rejected",
    ];
    ThisAttempt 413 => ["body_too_large"];
    ThisAttempt 415 => ["unsupported_media_type"];
    ThisAttempt 422 => [
        "invalid_mail_template",
        "invalid_runtime_switch",
        "not_acknowledgeable",
        "credential_login_rejected",
        "credential_login_challenge",
    ];
    ThisAttempt 429 => [
        "rate_limited",
        "credential_login_busy",
    ];
    // 能力检查在写入前拒绝本次尝试，同 ID 较早的未知结果留待确认。
    ThisAttempt 501 => ["feature_not_supported"];
    // 登录失败时未得到可导入的凭据，存储写入尚未开始。
    ThisAttempt 503 => ["credential_login_unavailable"];
    ReplayChecked 409 => [
        // 只有原围栏已无法通过，账本检查才足以排除尚未取锁的较早请求。
        "alert_stale",
        "stale_auction_state",
        "stale_lot_state",
        "stale_user_state",
        "rename_not_revertable",
        "mail_template_conflict",
        "runtime_switch_conflict",
        "already_disposed",
        "stale_settlement_return_state",
        "settlement_return_untrusted",
        "settlement_return_unbound",
        "settlement_return_assets_mismatch",
        "settlement_return_proof_required",
        "conflict",
    ];
}

/// 业务命令失败的证据；只有发送器校验过的回执可以解除此前未知。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AdminWriteFailure {
    /// 业务请求未发出，保留写前阶段的显示码。
    NotSent {
        /// 写前失败的稳定码。
        code: String,
    },
    /// 业务请求可能已生效，保留原命令供重试。
    OutcomeUnknown {
        /// 页面的稳定显示码。
        code: String,
        /// 页面可用的真实写响应状态。正文未完整收到时沿用原来的空状态。
        response_status: Option<u16>,
    },
    /// 实际业务写响应带有可识别归属及已验证的错误信封。
    VerifiedHttp(VerifiedWriteReceipt),
}

impl AdminWriteFailure {
    /// 写入尚未发出，例如表单校验或写前读取失败。
    pub(crate) fn not_sent(code: impl Into<String>) -> Self {
        Self::NotSent { code: code.into() }
    }

    /// 本次写入可能已经生效，仍须复用原命令 ID 和负载。
    pub(crate) fn outcome_unknown(code: impl Into<String>) -> Self {
        Self::OutcomeUnknown { code: code.into(), response_status: None }
    }

    /// 成功回执解码或核对失败时保留未决命令，并上报解码类别。
    pub(crate) fn invalid_response(error: impl ToString) -> Self {
        Self::outcome_unknown(invalid_response_error(error))
    }

    /// 页面可观察到的写响应状态，独立于该响应能否裁定命令。
    pub(crate) fn observed_response_status(&self) -> Option<u16> {
        match self {
            Self::NotSent { .. } => None,
            Self::OutcomeUnknown { response_status, .. } => *response_status,
            Self::VerifiedHttp(receipt) => Some(receipt.status()),
        }
    }

    /// 供页面、面板和外壳翻译的稳定码。
    pub(crate) fn display_code(&self) -> Cow<'_, str> {
        match self {
            Self::NotSent { code } | Self::OutcomeUnknown { code, .. } => Cow::Borrowed(code),
            Self::VerifiedHttp(receipt) => AdminApiFailure::normalize_http_code(
                Some(receipt.endpoint()),
                receipt.status(),
                receipt.protocol_code(),
            ),
        }
    }

    /// 只有对应的写回执才能锁定已陈旧的草稿；写前的同名错误仍可展示。
    pub(crate) fn is_verified_rejection(&self, code: &str) -> bool {
        matches!(self, Self::VerifiedHttp(receipt)
            if receipt.protocol_code() == code
                && AdminApiFailure::rejection_status(Some(receipt.endpoint()), code)
                    == Some(receipt.status()))
    }

    /// 只凭实际写回执裁定早先尝试，发送前失败只能裁定本次。
    pub(crate) fn classify(&self) -> Rejection {
        match self {
            Self::NotSent { .. } => Rejection::ThisAttempt,
            Self::OutcomeUnknown { .. } => Rejection::Unknown,
            Self::VerifiedHttp(receipt) => AdminApiFailure::rejection_for_status(
                Some(receipt.endpoint()),
                Some(receipt.status()),
                receipt.protocol_code(),
            )
            .unwrap_or(Rejection::Unknown),
        }
    }
}

/// 排除仅供浏览器内部使用的失败标记。
pub(super) fn valid_protocol_code(code: &str) -> bool {
    !code.starts_with(NOT_SENT_PREFIX)
        && !code.starts_with(WRITE_ENDPOINT_PREFIX)
        && code != "invalid_request"
}

/// Admin 浏览器请求的稳定失败投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminApiFailure {
    pub status: Option<u16>,
    pub code: String,
}

/// 只有契约规定的错误信封能证明命令被拒绝，可空字段也须显式出现。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminApiErrorResponse {
    code: String,
    message: String,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    // 只有对象或 null 是错误契约；异常 data 不能作为清除未知命令的证据。
    data: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(deserialize_with = "auction_contracts::deserialize_required_option")]
    request_id: Option<String>,
}

impl AdminApiErrorResponse {
    fn into_code(self) -> String {
        let Self { code, message, data, request_id } = self;
        // 只有稳定码进入浏览器状态或界面。
        let _ = (message, data, request_id);
        code
    }
}

/// HTTP 错误解码结果。保留信封是否有效，供业务写入单独裁定。
pub(super) struct DecodedHttpFailure {
    pub(super) status: u16,
    pub(super) protocol_code: Option<String>,
}

impl DecodedHttpFailure {
    /// 严格解码完整的正文。坏 UTF-8 只保留响应状态。
    pub(super) fn from_bytes(status: u16, body: &[u8]) -> Self {
        // Fetch 的文本解码会替换坏字节，坏编码的正文不能作为拒绝证据。
        match std::str::from_utf8(body) {
            Ok(body) => Self::from_body(status, body),
            Err(_error) => Self::invalid_body(status),
        }
    }

    pub(super) fn from_body(status: u16, body: &str) -> Self {
        // Serde 也能按字段顺序解码结构体，错误回执只接受具名的 JSON 对象。
        if !body.trim_ascii_start().starts_with('{') {
            return Self::invalid_body(status);
        }
        match serde_json::from_str::<AdminApiErrorResponse>(body) {
            Ok(payload) => Self { status, protocol_code: Some(payload.into_code()) },
            Err(_error) => Self::invalid_body(status),
        }
    }

    /// 已缓冲的响应正文无法读取或解码时，保留已收到的状态并记录解码故障。
    pub(super) fn invalid_body(status: u16) -> Self {
        report_browser_error(BrowserErrorClass::DtoDecode);
        Self { status, protocol_code: None }
    }

    fn into_api_failure(self) -> AdminApiFailure {
        let code = self.protocol_code.unwrap_or_else(|| format!("http_{}", self.status));
        AdminApiFailure::from_http(self.status, code)
    }
}

impl AdminApiFailure {
    pub(crate) fn client(code: impl ToString) -> Self {
        Self { status: None, code: code.to_string() }
    }

    /// 从原始字节解码错误回执，编码损坏时只保留 HTTP 状态。
    #[cfg(test)]
    pub(crate) fn from_http_bytes(status: u16, body: &[u8]) -> Self {
        DecodedHttpFailure::from_bytes(status, body).into_api_failure()
    }

    /// 解码失败回执。无法读出契约错误码时，只有 HTTP 状态可用于展示。
    #[cfg(test)]
    pub(crate) fn from_http_body(status: u16, body: &str) -> Self {
        DecodedHttpFailure::from_body(status, body).into_api_failure()
    }

    pub(crate) const fn is_conflict(&self) -> bool {
        matches!(self.status, Some(409))
    }

    /// 未知结果只能重试同一命令 ID 与原负载。错误状态本身不能证明本次未写入。
    pub(crate) fn outcome_unknown(&self) -> bool {
        Self::rejection(&self.code, self.status).is_none()
    }

    /// 只有经过账本核对且状态码吻合的拒绝，才能解除先前结果未知的命令。
    #[cfg(test)]
    pub(crate) fn resolves_prior_attempt(&self) -> bool {
        self.status.is_some()
            && Self::rejection(&self.code, self.status) == Some(Rejection::ReplayChecked)
    }

    /// 旧投影用例仍验证内部标记的读侧兼容。
    #[cfg(test)]
    fn at_write_endpoint(mut self, endpoint: AdminWriteEndpoint) -> Self {
        if let Some(status) = self.status {
            let code = Self::normalize_http_code(Some(endpoint), status, &self.code);
            self.code = format!("{WRITE_ENDPOINT_PREFIX}{}:{status}:{code}", endpoint.label());
        }
        self
    }

    /// 业务请求发出前失败。保留原稳定码供界面解释失败原因。
    pub(crate) fn not_sent_code(code: &str) -> String {
        // 准备请求的同名错误不能裁定写命令此前的尝试。
        let code = Self::display_code(code);
        if Self::definite_rejection(code) && !Self::rejection_after_replay_check(code) {
            code.to_owned()
        } else {
            format!("{NOT_SENT_PREFIX}{code}")
        }
    }

    /// 取适用于既有界面文案的码，去掉浏览器内部的阶段标记。
    pub(crate) fn display_code(code: &str) -> &str {
        let code = code.trim_prefix(NOT_SENT_PREFIX);
        Self::write_endpoint_code(code).map_or(code, |(_, _, display)| display)
    }

    /// 只认发出前的失败，以及本次尝试未产生也不会产生所请求效果的拒绝。
    pub(crate) fn definite_rejection(code: &str) -> bool {
        Self::rejection(code, None).is_some()
    }

    /// 回执须先经过同 ID 重放检查，且仍在途的原请求也不可能通过之后的状态校验。
    pub(crate) fn rejection_after_replay_check(code: &str) -> bool {
        Self::rejection(code, None) == Some(Rejection::ReplayChecked)
    }

    fn write_endpoint_code(code: &str) -> Option<(AdminWriteEndpoint, u16, &str)> {
        let (endpoint, receipt) = code.strip_prefix(WRITE_ENDPOINT_PREFIX)?.split_once(':')?;
        let (status, code) = receipt.split_once(':')?;
        Some((AdminWriteEndpoint::parse(endpoint)?, status.parse().ok()?, code))
    }

    fn rejection(code: &str, status: Option<u16>) -> Option<Rejection> {
        if status.is_none() && (code.starts_with(NOT_SENT_PREFIX) || code == "invalid_request") {
            return Some(Rejection::ThisAttempt);
        }
        let (endpoint, status, code) = match Self::write_endpoint_code(code) {
            Some((endpoint, received, code)) if status.is_none_or(|status| status == received) => {
                (Some(endpoint), Some(received), code)
            }
            Some(_) => return None,
            None => (None, status, code),
        };
        Self::rejection_for_status(endpoint, status, code)
    }

    fn rejection_for_status(
        endpoint: Option<AdminWriteEndpoint>,
        status: Option<u16>,
        code: &str,
    ) -> Option<Rejection> {
        let (expected_status, scope) = Self::rejection_rule(endpoint, code)?;
        if status.is_some_and(|status| status != expected_status) {
            return None;
        }
        Some(scope)
    }

    fn rejection_status(endpoint: Option<AdminWriteEndpoint>, code: &str) -> Option<u16> {
        Self::rejection_rule(endpoint, code).map(|(status, _)| status)
    }

    fn rejection_rule(
        endpoint: Option<AdminWriteEndpoint>,
        code: &str,
    ) -> Option<(u16, Rejection)> {
        if code == "state_conflict"
            && endpoint.is_some_and(AdminWriteEndpoint::is_versioned_mutation)
        {
            return Some((409, Rejection::ReplayChecked));
        }
        write_rejection_rule(code)
    }

    fn from_http(status: u16, code: String) -> Self {
        let code = match Self::normalize_http_code(None, status, &code) {
            Cow::Borrowed(_) => code,
            Cow::Owned(fallback) => fallback,
        };
        Self { status: Some(status), code }
    }

    fn normalize_http_code(
        endpoint: Option<AdminWriteEndpoint>,
        status: u16,
        code: &str,
    ) -> Cow<'_, str> {
        // 只允许完整的 413 契约信封证明正文在入口处被拒绝；非契约 413 的来源不可证。
        // 服务端只提供协议码，不能自行提供浏览器的未发送标记或端点标记。
        if !valid_protocol_code(code)
            || Self::rejection_status(endpoint, code).is_some_and(|expected| expected != status)
        {
            Cow::Owned(format!("http_{status}"))
        } else {
            Cow::Borrowed(code)
        }
    }
}

/// 退件页原有的显示码重读条件，由页面回调使用。
pub(crate) fn return_failure_needs_reload(code: &str) -> bool {
    code == "invalid_request" || AdminApiFailure::rejection_status(None, code).is_some()
}

/// 将同源错误 envelope 缩减为稳定错误码；正文无效时只保留 HTTP 状态兜底。
pub(crate) async fn response_failure(response: AdminResponse) -> AdminApiFailure {
    let failure = read_response_failure(&response).await;
    if failure.status == Some(401) && failure.code == "unauthenticated" {
        response.report_session_invalidation();
    }
    failure
}

/// 读取错误信封的稳定码与 HTTP 状态，供会话复核使用，避免再触发一次复核。
pub(super) async fn read_response_failure(response: &Response) -> AdminApiFailure {
    decode_http_failure(response).await.into_api_failure()
}

/// 读取原始正文并保留信封校验结果。
pub(super) async fn decode_http_failure(response: &Response) -> DecodedHttpFailure {
    let status = response.status();
    match response.binary().await {
        Ok(body) => DecodedHttpFailure::from_bytes(status, &body),
        Err(_error) => DecodedHttpFailure::invalid_body(status),
    }
}

/// `Result<_, String>` API 只返回稳定错误码，兼容现有页面状态而不泄漏底层错误正文。
pub(crate) async fn response_error(response: AdminResponse) -> String {
    response_failure(response).await.code
}

/// 请求构造失败只暴露稳定业务 code；底层构建错误不进入页面。
pub(crate) fn invalid_request_error(_error: impl ToString) -> String {
    "invalid_request".to_owned()
}

/// 响应解码失败时上报 DTO 解码类别，页面只看到稳定 code。
pub(crate) fn invalid_response_error(_error: impl ToString) -> String {
    report_browser_error(BrowserErrorClass::DtoDecode);
    "invalid_response".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{AdminApiFailure, AdminWriteEndpoint};

    #[test]
    fn delivery_guards_reject_this_attempt_without_clearing_an_earlier_unknown() {
        for code in ["delivery_recipient_renamed", "delivery_command_snapshot_unreadable"] {
            let body = serde_json::json!({
                "code": code, "message": "投递动作已拒绝", "data": null, "request_id": null,
            });
            let failure = AdminApiFailure::from_http_body(409, &body.to_string());
            assert!(!failure.outcome_unknown(), "{code}");
            assert!(!AdminApiFailure::rejection_after_replay_check(&failure.code), "{code}");
            assert!(AdminApiFailure::from_http_body(503, &body.to_string()).outcome_unknown());
        }
    }

    #[test]
    fn credential_import_rejections_only_resolve_this_attempt() {
        for (status, code) in [
            (400, "invalid_credential_kind"),
            (400, "invalid_credential_world"),
            (400, "invalid_credential_request"),
            (400, "invalid_credential_label"),
            (400, "invalid_credential_player_name"),
            (400, "invalid_credential_secret"),
            (409, "credential_conflict"),
            (422, "credential_login_rejected"),
            (422, "credential_login_challenge"),
            (429, "credential_login_busy"),
            (503, "credential_login_unavailable"),
        ] {
            let body = serde_json::json!({
                "code": code, "message": "凭据导入已拒绝", "data": null, "request_id": null,
            });
            let failure = AdminApiFailure::from_http_body(status, &body.to_string());
            assert!(!failure.outcome_unknown(), "{code}");
            assert!(!AdminApiFailure::rejection_after_replay_check(&failure.code), "{code}");
            assert!(AdminApiFailure::from_http_body(502, &body.to_string()).outcome_unknown());
        }
        let unavailable = AdminApiFailure::from_http_body(
            503,
            r#"{"code":"storage_unavailable","message":"凭据存储不可用","data":null,"request_id":null}"#,
        );
        assert!(unavailable.outcome_unknown());
    }

    #[test]
    fn only_complete_error_envelopes_can_be_used_as_rejection_evidence() {
        for (status, code) in [(413, "body_too_large"), (409, "mail_template_conflict")] {
            let complete = serde_json::json!({
                "code": code,
                "message": "请求已拒绝",
                "data": null,
                "request_id": null,
            });
            let verified = AdminApiFailure::from_http_body(status, &complete.to_string());
            assert!(!verified.outcome_unknown());
            assert_eq!(
                AdminApiFailure::rejection_after_replay_check(&verified.code),
                status == 409
            );

            for key in ["code", "message", "data", "request_id"] {
                let mut missing = complete.clone();
                missing.as_object_mut().unwrap().remove(key);
                let rejected = AdminApiFailure::from_http_body(status, &missing.to_string());
                assert!(rejected.outcome_unknown(), "缺少 {key}");
                assert!(!AdminApiFailure::definite_rejection(&rejected.code));
                assert!(!AdminApiFailure::rejection_after_replay_check(&rejected.code));
            }

            for (key, invalid) in [
                ("code", serde_json::json!(409)),
                ("message", serde_json::json!(null)),
                ("request_id", serde_json::json!(true)),
                ("extra", serde_json::json!(null)),
            ] {
                let mut malformed = complete.clone();
                malformed[key] = invalid;
                let rejected = AdminApiFailure::from_http_body(status, &malformed.to_string());
                assert!(rejected.outcome_unknown(), "字段不符 {key}");
                assert!(!AdminApiFailure::definite_rejection(&rejected.code));
                assert!(!AdminApiFailure::rejection_after_replay_check(&rejected.code));
            }

            let mut populated = complete;
            populated["request_id"] = serde_json::json!("admin-receipt");
            populated["data"] = serde_json::json!({"revision": 6});
            assert!(
                !AdminApiFailure::from_http_body(status, &populated.to_string()).outcome_unknown()
            );

            let positional = serde_json::json!([code, "请求已拒绝", null, null]);
            assert!(
                AdminApiFailure::from_http_body(status, &positional.to_string()).outcome_unknown()
            );
        }
    }

    #[test]
    fn only_unknown_outcomes_are_retried_verbatim() {
        assert!(AdminApiFailure::client("network_error").outcome_unknown());
        assert!(
            AdminApiFailure { status: Some(503), code: "outcome_unknown".to_owned() }
                .outcome_unknown()
        );
        assert!(
            AdminApiFailure { status: Some(502), code: "http_502".to_owned() }.outcome_unknown()
        );
        assert!(
            AdminApiFailure { status: Some(422), code: "unrecognized_rejection".to_owned() }
                .outcome_unknown()
        );
        assert!(!AdminApiFailure::client("invalid_request").outcome_unknown());
        // 明确的拒绝与冲突不是未知结果：修正输入或刷新后用新命令提交。
        for (status, code) in [(422, "invalid_mail_template"), (409, "mail_template_conflict")] {
            let failure = AdminApiFailure { status: Some(status), code: code.to_owned() };
            assert!(!failure.outcome_unknown());
        }
    }

    #[test]
    fn only_the_backend_413_envelope_unlocks_a_rejected_write() {
        let fixture = include_str!(
            "../../../auction-contracts/tests/fixtures/admin/mail-template-body-too-large.json"
        );
        let rejected = AdminApiFailure::from_http_body(413, fixture)
            .at_write_endpoint(AdminWriteEndpoint::MailTemplate);
        assert!(!rejected.outcome_unknown());
        assert_eq!(AdminApiFailure::display_code(&rejected.code), "body_too_large");
        assert!(!AdminApiFailure::rejection_after_replay_check(&rejected.code));
        assert!(AdminApiFailure::from_http(413, "http_413".to_owned()).outcome_unknown());
        let unavailable = AdminApiFailure::from_http(503, rejected.code);
        assert!(unavailable.outcome_unknown());
        assert!(!AdminApiFailure::definite_rejection(&unavailable.code));
    }

    fn version_conflict_receipt(status: u16, endpoint: AdminWriteEndpoint) -> AdminApiFailure {
        let body = match endpoint {
            AdminWriteEndpoint::UserMutation => include_str!(
                "../../../auction-contracts/tests/fixtures/admin/error-user-state-conflict.json"
            ),
            AdminWriteEndpoint::RulesPublish | AdminWriteEndpoint::AboutPublish => include_str!(
                "../../../auction-contracts/tests/fixtures/admin/error-document-state-conflict.json"
            ),
            _ => panic!("没有该端点的版本冲突样本"),
        };
        AdminApiFailure::from_http_body(status, body)
    }

    #[test]
    fn a_first_user_mutation_version_conflict_is_definite() {
        let receipt = version_conflict_receipt(409, AdminWriteEndpoint::UserMutation);
        let rejection = receipt.clone().at_write_endpoint(AdminWriteEndpoint::UserMutation);
        assert!(!rejection.outcome_unknown());
        assert!(rejection.resolves_prior_attempt());
        assert!(AdminApiFailure::definite_rejection(&rejection.code));
        assert_eq!(AdminApiFailure::display_code(&rejection.code), "state_conflict");
        assert!(receipt.outcome_unknown());
    }

    #[test]
    fn a_first_rules_or_about_publication_version_conflict_is_definite() {
        for endpoint in [AdminWriteEndpoint::RulesPublish, AdminWriteEndpoint::AboutPublish] {
            let rejection = version_conflict_receipt(409, endpoint).at_write_endpoint(endpoint);
            assert!(!rejection.outcome_unknown());
            assert!(rejection.resolves_prior_attempt());
            assert!(AdminApiFailure::definite_rejection(&rejection.code));
            for status in [400, 500] {
                let mismatch =
                    version_conflict_receipt(status, endpoint).at_write_endpoint(endpoint);
                assert!(mismatch.outcome_unknown());
                assert!(!AdminApiFailure::definite_rejection(&mismatch.code));
            }
        }
    }

    #[test]
    fn a_server_code_cannot_claim_the_endpoint_or_preflight_marker() {
        let tagged = AdminApiFailure::from_http(409, "state_conflict".to_owned())
            .at_write_endpoint(AdminWriteEndpoint::UserMutation);
        for code in [tagged.code, AdminApiFailure::not_sent_code("network_error")] {
            let forged = AdminApiFailure::from_http(409, code);
            assert!(forged.outcome_unknown());
            assert!(!AdminApiFailure::definite_rejection(&forged.code));
        }
    }

    #[test]
    fn reversible_rejections_do_not_resolve_an_earlier_attempt() {
        for code in [
            "rename_name_taken",
            "invalid_lot_state",
            "stale_recovery_state",
            "stale_delivery_state",
            "stale_world_state",
            "not_acknowledgeable",
            "delivery_sent_sighted",
            "settlement_obligation_ineligible",
            "settlement_return_action_conflict",
            "settlement_return_resend_limit",
            "settlement_return_obligation_ineligible",
            "settlement_return_send_unresolved",
            "settlement_return_observation_required",
        ] {
            assert!(AdminApiFailure::definite_rejection(code), "{code}");
            assert!(!AdminApiFailure::rejection_after_replay_check(code), "{code}");
        }
        for body in [
            include_str!(
                "../../../auction-contracts/tests/fixtures/admin/error-stale-delivery-state.json"
            ),
            include_str!(
                "../../../auction-contracts/tests/fixtures/admin/error-stale-delivery-disposition.json"
            ),
        ] {
            let failure = AdminApiFailure::from_http_body(409, body);
            assert_eq!(failure.code, "stale_delivery_state");
            assert!(!failure.outcome_unknown());
            assert!(!failure.resolves_prior_attempt());
        }
        let conflict = AdminApiFailure::from_http_body(
            409,
            include_str!(
                "../../../auction-contracts/tests/fixtures/admin/error-rename-name-taken.json"
            ),
        )
        .at_write_endpoint(AdminWriteEndpoint::RenameRevert);
        assert!(!conflict.outcome_unknown());
        assert!(!AdminApiFailure::rejection_after_replay_check(&conflict.code));
    }

    #[test]
    fn reclassified_rejections_require_the_exact_http_status() {
        for (expected, code) in [
            (400, "invalid_json"),
            (400, "invalid_auction_intake"),
            (400, "unsupported_equipment_source"),
            (403, "manual_early_settlement_disabled"),
            (404, "not_found"),
            (404, "settlement_return_not_found"),
            (409, "stale_lot_state"),
            (409, "command_conflict"),
            (409, "credential_conflict"),
            (409, "stale_credential_state"),
            (409, "auction_intake_unavailable"),
            (409, "seller_blacklisted"),
            (409, "equipment_conflict"),
            (422, "not_acknowledgeable"),
            (422, "credential_login_rejected"),
            (422, "credential_login_challenge"),
            (429, "rate_limited"),
            (429, "credential_login_busy"),
        ] {
            let rejection = AdminApiFailure::from_http(expected, code.to_owned());
            assert!(!rejection.outcome_unknown(), "{code}");
            assert_eq!(rejection.code, code);
            for status in [418, 500] {
                let mismatch = AdminApiFailure::from_http(status, code.to_owned());
                assert!(mismatch.outcome_unknown(), "{code} {status}");
                assert!(!AdminApiFailure::definite_rejection(&mismatch.code), "{code} {status}");
            }
        }
    }

    #[test]
    fn only_verified_replay_checked_rejections_can_clear_unknown_history() {
        for (code, body) in [
            (
                "alert_stale",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-alert-stale.json"
                ),
            ),
            (
                "stale_auction_state",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-stale-auction-state.json"
                ),
            ),
            (
                "stale_lot_state",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-stale-lot-state.json"
                ),
            ),
            (
                "stale_user_state",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-stale-user-state.json"
                ),
            ),
            (
                "rename_not_revertable",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-rename-not-revertable.json"
                ),
            ),
            (
                "mail_template_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-mail-template-conflict.json"
                ),
            ),
            (
                "runtime_switch_conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-runtime-switch-conflict.json"
                ),
            ),
            (
                "already_disposed",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-already-disposed.json"
                ),
            ),
            (
                "stale_settlement_return_state",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-stale-settlement-return-state.json"
                ),
            ),
            (
                "settlement_return_untrusted",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-settlement-return-untrusted.json"
                ),
            ),
            (
                "settlement_return_unbound",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-settlement-return-unbound.json"
                ),
            ),
            (
                "settlement_return_assets_mismatch",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-settlement-return-assets-mismatch.json"
                ),
            ),
            (
                "settlement_return_proof_required",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-settlement-return-proof-required.json"
                ),
            ),
            (
                "conflict",
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/error-realtime-conflict.json"
                ),
            ),
        ] {
            let rejection = AdminApiFailure::from_http_body(409, body);
            assert_eq!(rejection.code, code);
            assert!(!rejection.outcome_unknown(), "{code}");
            assert!(rejection.resolves_prior_attempt(), "{code}");
            assert!(AdminApiFailure::rejection_after_replay_check(&rejection.code));
            assert!(!AdminApiFailure::client(&rejection.code).resolves_prior_attempt());
            for status in [400, 503] {
                let mismatch = AdminApiFailure::from_http_body(status, body);
                assert!(mismatch.outcome_unknown(), "{code} {status}");
                assert!(!mismatch.resolves_prior_attempt());
            }
        }

        let rejection = AdminApiFailure::from_http_body(
            409,
            include_str!("../../../auction-contracts/tests/fixtures/admin/error-alert-stale.json"),
        );
        let not_sent = AdminApiFailure::not_sent_code(&rejection.code);
        assert!(AdminApiFailure::definite_rejection(&not_sent));
        assert_eq!(AdminApiFailure::display_code(&not_sent), "alert_stale");
        assert!(!AdminApiFailure::rejection_after_replay_check(&not_sent));
        for code in ["invalid_alert_command", "stale_credential_state", "rate_limited"] {
            assert!(AdminApiFailure::definite_rejection(code));
            assert!(!AdminApiFailure::rejection_after_replay_check(code));
        }
        for status in [400, 500] {
            let mismatch = AdminApiFailure::from_http(status, rejection.code.clone());
            assert!(mismatch.outcome_unknown());
            assert!(!mismatch.resolves_prior_attempt());
            assert!(!AdminApiFailure::rejection_after_replay_check(&mismatch.code));
        }
    }

    #[test]
    fn a_preflight_error_is_definite_only_before_dispatch() {
        for code in ["request_timeout", "network_error", "invalid_response", "http_503"] {
            let not_sent = AdminApiFailure::not_sent_code(code);
            assert!(!AdminApiFailure::client(&not_sent).outcome_unknown());
            assert_eq!(AdminApiFailure::display_code(&not_sent), code);
            assert!(AdminApiFailure::client(code).outcome_unknown());
            assert!(AdminApiFailure::from_http(400, not_sent).outcome_unknown());
        }
    }

    #[test]
    fn a_rejection_with_non_object_data_cannot_clear_unknown_history() {
        let body = include_str!(
            "../../../auction-contracts/tests/fixtures/admin/error-mail-template-conflict.json"
        );
        let valid = AdminApiFailure::from_http_body(409, body);
        assert!(valid.resolves_prior_attempt());
        let mut envelope: serde_json::Value = serde_json::from_str(body).expect("错误信封");
        for data in [serde_json::json!(true), serde_json::json!(["fixture"])] {
            envelope["data"] = data;
            let failure = AdminApiFailure::from_http_body(409, &envelope.to_string());
            assert!(failure.outcome_unknown());
            assert!(!failure.resolves_prior_attempt());
        }
    }
}
