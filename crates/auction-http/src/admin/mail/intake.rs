//! Admin 入站邮件列表与详情的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminMailIntakeDelivery, AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakeMetadata,
    AdminMailIntakeSummary,
};
use auction_application::core::ApplicationError;
use auction_application::mail::{MailDisplaySnapshot, MailEquipmentDiagnosticSnapshot};
use auction_contracts::{
    AdminMailDisplayResponse, AdminMailEquipmentDiagnosticResponse,
    AdminMailIntakeDeliveryResponse, AdminMailIntakeDetailResponse, AdminMailIntakeListResponse,
    AdminMailIntakeMetadataResponse, AdminMailIntakeSummaryResponse, AdminMailPaginationResponse,
    ApiResponse,
};
use auction_time::Timestamp;
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{feature_unavailable, map_error};
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::timestamp;
use crate::query::{parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

/// 读取 Admin 入站邮件安全摘要列表。
pub(crate) async fn list_intakes(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let mut query = match parse_query(
        &uri,
        &[
            "mailbox_key",
            "world_id",
            "kind",
            "status",
            "decision",
            "from",
            "to",
            "page",
            "page_size",
        ],
    ) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return query_rejection().into_response(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(()) => return query_rejection().into_response(),
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        20,
        Some(auction_application::admin::MAX_ADMIN_MAIL_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(()) => return query_rejection().into_response(),
    };
    let received_from = match parse_time(query.get("from")) {
        Ok(value) => value,
        Err(()) => return query_rejection().into_response(),
    };
    let received_to = match parse_time(query.get("to")) {
        Ok(value) => value,
        Err(()) => return query_rejection().into_response(),
    };
    let request = AdminMailIntakeFilter {
        mailbox_key: query.remove("mailbox_key"),
        world_id: query.remove("world_id"),
        kind: query.remove("kind"),
        status: query.remove("status"),
        decision: query.remove("decision"),
        received_from,
        received_to,
        page,
        page_size,
    };
    if let Err(error) = request.validate() {
        return map_error(ApplicationError::from(error));
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.list_intakes(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "邮件入站列表已加载",
            AdminMailIntakeListResponse {
                intakes: result.items.into_iter().map(map_summary).collect(),
                pagination: AdminMailPaginationResponse {
                    page: result.page,
                    page_size: result.page_size,
                    total: result.total,
                },
            },
        )),
    )
        .into_response()
}

/// 读取单条 Admin 入站邮件详情。
pub(crate) async fn detail_intake(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(intake_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return query_rejection().into_response();
    }
    let Ok(intake_id) = Uuid::parse_str(&intake_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_intake_id", "邮件入站 ID 无效")
            .into_response();
    };
    if intake_id.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_intake_id", "邮件入站 ID 无效")
            .into_response();
    }
    let Some(port) = state.admin_mail() else {
        return feature_unavailable();
    };
    let result = match port.intake_detail(intake_id).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let Some(result) = result else {
        return not_found_error().into_response();
    };
    if !result.summary.validate()
        || !result.metadata.validate()
        || result.deliveries.iter().any(|delivery| !delivery.validate())
    {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("邮件入站详情已加载", map_detail(result))),
    )
        .into_response()
}

fn parse_time(value: Option<&String>) -> Result<Option<Timestamp>, ()> {
    value.map(|value| Timestamp::parse_from_rfc3339(value).map_err(|_| ())).transpose()
}

fn map_summary(summary: AdminMailIntakeSummary) -> AdminMailIntakeSummaryResponse {
    // 列表和详情复用同一读时投影，避免把旧行的原主题送到客户端。
    let subject = summary
        .sender
        .as_deref()
        .zip(summary.subject.as_deref())
        .map(|(sender, subject)| MailDisplaySnapshot::redact_subject(sender, subject, &[]));
    AdminMailIntakeSummaryResponse {
        id: summary.id.to_string(),
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: timestamp(summary.received_at),
        processed_at: timestamp(summary.processed_at),
        sender: summary.sender,
        subject,
        has_message_display: summary.has_message_display,
    }
}

fn map_detail(detail: AdminMailIntakeDetail) -> AdminMailIntakeDetailResponse {
    let summary = map_summary(detail.summary);
    AdminMailIntakeDetailResponse {
        id: summary.id,
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: summary.received_at,
        processed_at: summary.processed_at,
        sender: summary.sender,
        subject: summary.subject,
        metadata: map_metadata(detail.metadata),
        deliveries: detail.deliveries.into_iter().map(map_delivery).collect(),
        message_display: detail.message_display.map(map_display),
        equipment_diagnostic: detail.equipment_diagnostic.map(map_equipment_diagnostic),
    }
}

fn map_delivery(delivery: AdminMailIntakeDelivery) -> AdminMailIntakeDeliveryResponse {
    AdminMailIntakeDeliveryResponse {
        id: delivery.id.to_string(),
        operation: delivery.operation,
        recipient: delivery.recipient,
        status: delivery.status,
        fencing_version: delivery.fencing_version,
        attempt_count: delivery.attempt_count,
        failure_code: delivery.failure_code,
        reason_code: delivery.reason_code,
        external_reference: delivery.external_reference,
    }
}

fn map_display(display: MailDisplaySnapshot) -> AdminMailDisplayResponse {
    AdminMailDisplayResponse {
        schema_version: display.schema_version,
        redaction_version: display.redaction_version,
        body: MailDisplaySnapshot::redact_stored_body(&display.body),
        attachment_ids: display.attachment_ids,
        credits: display.credits,
        cod: display.cod,
    }
}

fn map_equipment_diagnostic(
    diagnostic: MailEquipmentDiagnosticSnapshot,
) -> AdminMailEquipmentDiagnosticResponse {
    AdminMailEquipmentDiagnosticResponse {
        schema_version: diagnostic.schema_version,
        equipment_id: diagnostic.equipment_id,
        attachment_position: diagnostic.attachment_position,
        stage: diagnostic.stage.as_str().to_owned(),
        category: diagnostic.category.as_str().to_owned(),
        detail_code: diagnostic.detail_code,
        http_status: diagnostic.http_status,
        elapsed_millis: diagnostic.elapsed_millis,
        evidence_id: diagnostic.evidence_id.map(|id| id.to_string()),
        evidence_status: diagnostic.evidence_status.as_str().to_owned(),
        retention_error: diagnostic.retention_error,
    }
}

fn map_metadata(metadata: AdminMailIntakeMetadata) -> AdminMailIntakeMetadataResponse {
    AdminMailIntakeMetadataResponse {
        auction_id: metadata.auction_id,
        submission_id: metadata.submission_id,
        user_id: metadata.user_id,
        added_count: metadata.added_count,
        existing_count: metadata.existing_count,
        preview_revision: metadata.preview_revision,
        attachment_count: metadata.attachment_count,
        owner_result_code: metadata.owner_result_code,
        reason_code: metadata.reason_code,
        warning_codes: metadata.warning_codes,
        delivery_ids: metadata.delivery_ids,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminMailCommandPort, AdminMailDashboardSnapshot, AdminMailIntakeDetail,
        AdminMailIntakeFilter, AdminMailIntakeMetadata, AdminMailIntakePage,
        AdminMailIntakeSummary, AdminMailPollState, AdminMailPollStateRequest,
        AdminMailPollTriggerCommand, AdminMailPollingCommand, AdminMailReader,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::mail::MailDisplaySnapshot;
    use auction_contracts::{AdminMailIntakeDetailResponse, AdminMailIntakeSummaryResponse};
    use auction_time::Timestamp;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use super::{map_detail, map_summary};
    use crate::server::contract_fixtures::{AdminFixturePorts, admin_get, assert_http, time};

    #[test]
    fn admin_mail_list_and_detail_serialize_only_safe_subjects() {
        let now = Timestamp::from_timestamp(1_800_000_000, 0).unwrap();
        let colon_key_subject = format!("补充说明：登录 key {}:", " ".repeat(31));
        let whitespace_key_subject = format!("补充说明：登录 key {}", " ".repeat(32));
        for (raw, expected) in [
            (colon_key_subject.as_str(), "补充说明：登录 key[REDACTED]"),
            (whitespace_key_subject.as_str(), "补充说明：登录 key[REDACTED]"),
            ("补充说明：LoGiN KeY =短 值", "补充说明：LoGiN KeY[REDACTED]"),
            ("key: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "key: [REDACTED]"),
            (
                "https://example.invalid/equip/123/0123abcdef",
                "https://example.invalid/equip/123/[REDACTED]",
            ),
            ("登录 key = 手工 凭据可含空格! 并非系统签发的密钥形态", "登录 key[REDACTED]"),
            ("附件 3D2F1E0C9B", "附件 [REDACTED]"),
            (
                "补充说明：LoGiN KeY 多字节 夹杂 空格 与标点! 这不是可通过形态识别的值",
                "补充说明：LoGiN KeY[REDACTED]",
            ),
            ("补充说明：登录 key 多字节 夹杂 空格 与标点!", "补充说明：登录 key[REDACTED]"),
            ("补充说明：登录 key  中 abcdefghijklmnopqrstuvwxyz!", "补充说明：登录 key[REDACTED]"),
            ("补充说明：LoGiN KeY 短 值", "补充说明：LoGiN KeY[REDACTED]"),
            ("补充说明：登录 key", "补充说明：登录 key"),
            (
                "买家在看 xlogin key 文档和说明 这是一句普通主题",
                "买家在看 xlogin key 文档和说明 这是一句普通主题",
            ),
            ("如何修复 login keyboard", "如何修复 login keyboard"),
            ("拍卖问题", "拍卖问题"),
        ] {
            let summary = AdminMailIntakeSummary {
                id: Uuid::new_v4(),
                mailbox_key: "main".to_owned(),
                external_mail_id: "9001".to_owned(),
                world_id: "main".to_owned(),
                kind: "unknown".to_owned(),
                status: "accepted".to_owned(),
                decision_code: "ignored".to_owned(),
                received_at: now,
                processed_at: now,
                sender: Some("Player".to_owned()),
                subject: Some(raw.to_owned()),
                has_message_display: true,
            };
            let list_json = serde_json::to_vec(&map_summary(summary.clone())).unwrap();
            let list: AdminMailIntakeSummaryResponse = serde_json::from_slice(&list_json).unwrap();
            assert_eq!(list.subject.as_deref(), Some(expected));

            let detail_json = serde_json::to_vec(&map_detail(AdminMailIntakeDetail {
                summary,
                metadata: AdminMailIntakeMetadata {
                    auction_id: String::new(),
                    submission_id: String::new(),
                    user_id: String::new(),
                    added_count: 0,
                    existing_count: 0,
                    preview_revision: 0,
                    attachment_count: 0,
                    owner_result_code: String::new(),
                    reason_code: String::new(),
                    warning_codes: Vec::new(),
                    delivery_ids: Vec::new(),
                },
                deliveries: Vec::new(),
                message_display: Some(MailDisplaySnapshot {
                    schema_version: 1,
                    redaction_version: 1,
                    body: raw.to_owned(),
                    attachment_ids: vec!["123".to_owned()],
                    credits: 0,
                    cod: 0,
                }),
                equipment_diagnostic: None,
            }))
            .unwrap();
            let detail: AdminMailIntakeDetailResponse =
                serde_json::from_slice(&detail_json).unwrap();
            assert_eq!(detail.subject.as_deref(), Some(expected));
            assert_eq!(detail.message_display.unwrap().body, expected);
        }
    }

    struct FixtureIntakes {
        items: Vec<AdminMailIntakeSummary>,
        details: Vec<AdminMailIntakeDetail>,
    }

    impl AdminMailReader for FixtureIntakes {
        fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot> {
            panic!("入站样本不查询仪表盘")
        }

        fn poll_state(
            &self,
            request: AdminMailPollStateRequest,
        ) -> ApplicationFuture<'_, AdminMailPollState> {
            assert_eq!(request.mailbox_key, "main");
            Box::pin(async {
                Ok(AdminMailPollState {
                    mailbox_key: request.mailbox_key,
                    cursor_value: Some("9002".to_owned()),
                    last_poll_at: time(),
                    next_poll_at: time() + auction_time::Duration::seconds(15),
                    consecutive_failures: 0,
                    paused: false,
                    state_version: 3,
                    updated_at: time(),
                })
            })
        }

        fn list_intakes(
            &self,
            request: AdminMailIntakeFilter,
        ) -> ApplicationFuture<'_, AdminMailIntakePage> {
            Box::pin(async move {
                let items = self
                    .items
                    .iter()
                    .filter(|row| request.kind.as_ref().is_none_or(|kind| kind == &row.kind))
                    .cloned()
                    .collect::<Vec<_>>();
                let total = i64::try_from(items.len()).expect("记录数");
                Ok(AdminMailIntakePage {
                    items,
                    total,
                    page: request.page,
                    page_size: request.page_size,
                })
            })
        }

        fn intake_detail(
            &self,
            intake_id: Uuid,
        ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>> {
            Box::pin(async move {
                Ok(self.details.iter().find(|detail| detail.summary.id == intake_id).cloned())
            })
        }
    }

    impl AdminMailCommandPort for FixtureIntakes {
        fn set_polling_paused(
            &self,
            _command: AdminMailPollingCommand,
        ) -> ApplicationFuture<'_, AdminMailPollState> {
            panic!("入站样本不执行轮询命令")
        }

        fn trigger_poll(
            &self,
            _command: AdminMailPollTriggerCommand,
        ) -> ApplicationFuture<'_, AdminMailPollState> {
            panic!("入站样本不执行轮询命令")
        }
    }

    #[tokio::test]
    async fn intake_rekey_contract_fixtures() {
        let registration = AdminMailIntakeSummary {
            id: Uuid::from_u128(0x4000_8000_0000_0000_0021),
            mailbox_key: "main".to_owned(),
            external_mail_id: "9001".to_owned(),
            world_id: "main".to_owned(),
            kind: "registration".to_owned(),
            status: "accepted".to_owned(),
            decision_code: "registration_accepted".to_owned(),
            received_at: time(),
            processed_at: time(),
            sender: Some("ExamplePlayer".to_owned()),
            subject: Some("register".to_owned()),
            has_message_display: true,
        };
        let rekey = AdminMailIntakeSummary {
            id: Uuid::from_u128(0x4000_8000_0000_0000_0022),
            external_mail_id: "9002".to_owned(),
            kind: "rekey".to_owned(),
            decision_code: "rekey_accepted".to_owned(),
            sender: None,
            subject: None,
            has_message_display: false,
            ..registration.clone()
        };
        let detail_url = format!("/api/v1/admin/mail/intake/{}", rekey.id);
        let state = AdminFixturePorts {
            mail: Some(Arc::new(FixtureIntakes {
                items: vec![rekey.clone(), registration],
                details: vec![intake_detail_without_message(rekey)],
            })),
            ..Default::default()
        }
        .state();
        polling_contract_fixtures(state.clone()).await;
        for (name, url) in [
            ("admin/mail-intake", "/api/v1/admin/mail/intake?page_size=50"),
            ("admin/mail-intake-rekey", "/api/v1/admin/mail/intake?kind=rekey&page_size=50"),
            ("admin/mail-intake-detail", &detail_url),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), url).await).await;
        }
    }

    #[tokio::test]
    async fn legacy_intake_display_contract_fixtures() {
        use crate::server::contract_fixtures::{response_json, wire_fixture};

        let redacted = "[REDACTED]";
        let intake_id = |suffix: u128| Uuid::from_u128(0x4000_8000_0000_0000_0000 + suffix);
        let returned_subject =
            "[RTS] Auction payment request [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]";
        let near_limit = format!("          {} login key ", "x".repeat(491));
        let at_limit = format!("         {} login key ", "x".repeat(492));
        let padded_return = format!("{returned_subject} ");
        // 替身交付已完成的存储投影。原始字段取自 B3 的数据库用例，512 字节边界沿用其脱敏再裁剪的构造。
        let mut items = [
            (0x26, "9026", "Player", "Player", at_limit.as_str()),
            (0x25, "9025", "Player", "Player", near_limit.as_str()),
            (0x24, "9024", "MoogleMail ", redacted, returned_subject),
            (0x23, "9023", "MoogleMail", "MoogleMail", padded_return.as_str()),
            (0x22, "9022", "MoogleMail", "MoogleMail", returned_subject),
            (0x19, "9019", "Player", "Player", "补充说明：登录 key   "),
            (0x18, "9018", "Player ", "Player", "拍卖问题 \u{3000}"),
            (0x12, "9012", "Player", "Player", "附件 3D2F1E0C9B"),
        ]
        .into_iter()
        .map(|(id, mid, sender, display_sender, subject)| AdminMailIntakeSummary {
            id: intake_id(id),
            mailbox_key: "main".to_owned(),
            external_mail_id: mid.to_owned(),
            world_id: "main".to_owned(),
            kind: "unknown".to_owned(),
            status: "rejected".to_owned(),
            decision_code: "unknown_subject".to_owned(),
            received_at: time(),
            processed_at: time(),
            sender: Some(display_sender.to_owned()),
            subject: Some(MailDisplaySnapshot::display_stored_subject(sender, subject)),
            has_message_display: false,
        })
        .collect::<Vec<_>>();
        // 无关联投递的历史行可以同时含有这六个展示占位。
        let all_redacted = AdminMailIntakeSummary {
            id: intake_id(0x27),
            mailbox_key: redacted.to_owned(),
            external_mail_id: redacted.to_owned(),
            world_id: redacted.to_owned(),
            decision_code: redacted.to_owned(),
            sender: Some(redacted.to_owned()),
            subject: Some(MailDisplaySnapshot::display_stored_subject("\t\u{3000}", "多行\n主题")),
            ..items[0].clone()
        };
        items.insert(0, all_redacted);
        let detail_ids = [0x27, 0x19, 0x26, 0x18].map(intake_id);
        let details = detail_ids
            .iter()
            .map(|id| {
                let summary = items.iter().find(|row| row.id == *id).expect("详情对应的列表记录");
                intake_detail_without_message(summary.clone())
            })
            .collect();
        let state = AdminFixturePorts {
            mail: Some(Arc::new(FixtureIntakes { items, details })),
            ..Default::default()
        }
        .state();
        let list = admin_get(state.clone(), "/api/v1/admin/mail/intake?page_size=20").await;
        assert_http("admin/mail-intake-legacy", StatusCode::OK, list).await;

        let expected: [serde_json::Value; 4] =
            serde_json::from_value(wire_fixture("admin/mail-intake-legacy-detail"))
                .expect("四种历史入站邮件详情样本");
        for (id, expected) in detail_ids.into_iter().zip(expected) {
            let path = format!("/api/v1/admin/mail/intake/{id}");
            let response = admin_get(state.clone(), &path).await;
            assert_eq!(response_json(StatusCode::OK, response).await, expected, "{path}");
        }
    }

    fn intake_detail_without_message(summary: AdminMailIntakeSummary) -> AdminMailIntakeDetail {
        AdminMailIntakeDetail {
            summary,
            metadata: AdminMailIntakeMetadata {
                auction_id: String::new(),
                submission_id: String::new(),
                user_id: String::new(),
                added_count: 0,
                existing_count: 0,
                preview_revision: 0,
                attachment_count: 0,
                owner_result_code: String::new(),
                reason_code: String::new(),
                warning_codes: Vec::new(),
                delivery_ids: Vec::new(),
            },
            deliveries: Vec::new(),
            message_display: None,
            equipment_diagnostic: None,
        }
    }

    async fn polling_contract_fixtures(state: crate::state::AdminApiState) {
        use crate::server::contract_fixtures::{assert_wire, response_json};

        let path = "/api/v1/admin/mail?mailbox_key=main";
        assert_http("admin/mail", StatusCode::OK, admin_get(state.clone(), path).await).await;

        let sent = Timestamp::now();
        let response = admin_get(state, &format!("{path}&include_diagnostics=true")).await;
        let received = Timestamp::now();
        let mut body = response_json(StatusCode::OK, response).await;
        let observed = &mut body["data"]["polling"]["diagnostics"]["observed_at"];
        let observed_at =
            Timestamp::parse_from_rfc3339(observed.as_str().expect("观测时间")).expect("RFC 3339");
        let sent = Timestamp::from_timestamp_micros(sent.timestamp_micros()).unwrap();
        assert!((sent..=received).contains(&observed_at));
        // 核对生产时钟之后，仅归一化动态观测时刻。
        *observed = crate::mapping::timestamp(time()).into();
        assert_wire("admin/mail-diagnostics", body);
    }
}
