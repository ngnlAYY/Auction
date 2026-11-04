//! 论坛目标资料联合证明：固定 UID GET、资料布局及目标 MID 一致，不读取页头登录身份。

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics as safe_diagnostics;

use std::collections::BTreeMap;

use auction_application::{
    ForumPmError, ForumTargetProfileEvidence, InternalEvidenceError, valid_external_positive_id,
    valid_mail_player_name,
};
use scraper::{ElementRef, Node};
use sha2::{Digest, Sha256};

use super::*;

#[derive(Clone, Copy)]
enum ProfileStage {
    Client,
    Get,
    Owner,
    NameLayout,
    OptionsLayout,
    ContainerLayout,
    NameShape,
    NameValue,
    MidLayout,
    MidQuery,
    MidBinding,
}

impl ProfileStage {
    const fn diagnostic_phase(self) -> &'static str {
        match self {
            Self::Client => "validate_client_configuration",
            Self::Get => "fetch_profile",
            Self::Owner => "validate_profile_owner",
            Self::NameLayout => "parse_name_layout",
            Self::OptionsLayout => "parse_options_layout",
            Self::ContainerLayout => "parse_container_layout",
            Self::NameShape => "validate_name_shape",
            Self::NameValue => "validate_name_value",
            Self::MidLayout => "parse_mid_layout",
            Self::MidQuery => "parse_mid_query",
            Self::MidBinding => "validate_mid_binding",
        }
    }
}

// 只记录闭集阶段与应用错误；目标 UID、名称、URL 和原始资料页都不进入日志。
fn profile_failure(stage: ProfileStage, error: InternalEvidenceError) -> InternalEvidenceError {
    safe_diagnostics::warn(
        "forum_profile",
        stage.diagnostic_phase(),
        safe_diagnostics::project(&error),
        DiagnosticContext::default(),
    );
    error
}

// ForumPmError 全部为不携带文本的 unit variant；仅输出固定分类，不格式化传输错误。
fn profile_get_failure(error: ForumPmError) -> InternalEvidenceError {
    safe_diagnostics::warn(
        "forum_profile",
        ProfileStage::Get.diagnostic_phase(),
        safe_diagnostics::project(&error),
        DiagnosticContext::default(),
    );
    InternalEvidenceError::Unavailable
}

impl EhentaiClient {
    pub(in crate::clients::ehentai) async fn read_target_profile(
        &self,
        target_uid: &str,
        cookie: &EhentaiCookie,
    ) -> Result<ForumTargetProfileEvidence, InternalEvidenceError> {
        self.validate_pm_client().map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_profile",
                "validate_client",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            profile_failure(ProfileStage::Client, InternalEvidenceError::Unavailable)
        })?;
        if !valid_external_positive_id(target_uid) {
            return Err(InternalEvidenceError::InvalidRequest);
        }
        let target = self
            .index_url(&[("showuser", target_uid)], EhentaiOperation::CheckSession)
            .map_err(|diagnostic_cause| {
                safe_diagnostics::warn(
                    "forum_profile",
                    "build_profile_target",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                InternalEvidenceError::InvalidRequest
            })?;
        // 仅 GET 资料；复用强制不重定向的安全池，不经过发帖或 PM 准备流程。
        let source = self
            .pm_get(
                target.clone(),
                cookie,
                auction_application::ForumWorld::Shared,
                auction_application::ForumOperation::Profile,
            )
            .await
            .map_err(profile_get_failure)?;
        let name = parse_target_profile(&source, &target, target_uid)?;
        self.check_ownership(cookie).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_profile",
                "check_owner",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            profile_failure(ProfileStage::Owner, InternalEvidenceError::Revoked)
        })?;
        let observed_at = chrono::Utc::now();
        let protocol_version = "forum_target_profile_mid_v1";
        let mut hash = Sha256::new();
        let observed_text = observed_at.to_rfc3339();
        for value in [protocol_version, target_uid, name.as_str(), observed_text.as_str()] {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        Ok(ForumTargetProfileEvidence {
            target_uid: target_uid.to_owned(),
            player_name: name,
            observed_at,
            protocol_version,
            digest: hash.finalize().into(),
        })
    }
}

fn select_one<'a>(
    document: &'a Html,
    query: &str,
) -> Result<ElementRef<'a>, InternalEvidenceError> {
    let selector = Selector::parse(query).map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_profile",
            "build_profile_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        InternalEvidenceError::Unproven
    })?;
    let mut nodes = document.select(&selector);
    let node = nodes.next().ok_or(InternalEvidenceError::Unproven)?;
    if nodes.next().is_some() {
        return Err(InternalEvidenceError::Unproven);
    }
    Ok(node)
}

fn borderwrap(node: ElementRef<'_>) -> Option<ElementRef<'_>> {
    node.ancestors().filter_map(ElementRef::wrap).find(|node| {
        node.value().name() == "div" && node.value().classes().any(|class| class == "borderwrap")
    })
}

fn parse_target_profile(
    source: &str,
    target: &Url,
    uid: &str,
) -> Result<String, InternalEvidenceError> {
    let fail = |stage| profile_failure(stage, InternalEvidenceError::Unproven);
    let document = Html::parse_document(source);
    let name = select_one(&document, "#profilename")
        .map_err(|error| profile_failure(ProfileStage::NameLayout, error))?;
    let options = select_one(&document, "#profile-options")
        .map_err(|error| profile_failure(ProfileStage::OptionsLayout, error))?;
    let name_frame = borderwrap(name).ok_or_else(|| fail(ProfileStage::ContainerLayout))?;
    if borderwrap(options).map(|node| node.id()) != Some(name_frame.id()) {
        return Err(fail(ProfileStage::ContainerLayout));
    }
    let container = name_frame
        .parent()
        .and_then(ElementRef::wrap)
        .ok_or_else(|| fail(ProfileStage::ContainerLayout))?;
    if container.value().name() != "div"
        || container.attr("class").is_some()
        || container.parent().and_then(ElementRef::wrap).is_none_or(|node| {
            node.value().name() != "div" || !node.value().classes().any(|class| class == "page")
        })
    {
        return Err(fail(ProfileStage::ContainerLayout));
    }
    // 只接受已观察的 FONT/文本名称；不把隐藏脚本、图像替代文字当显示名。
    for node in name.descendants().skip(1) {
        match node.value() {
            Node::Text(_) => {}
            Node::Element(value)
                if value.name() == "font"
                    && value.attr("style").is_none()
                    && value.attr("hidden").is_none() => {}
            _ => return Err(fail(ProfileStage::NameShape)),
        }
    }
    let full_name = name.text().collect::<String>().trim().to_owned();
    if !valid_mail_player_name(&full_name) {
        return Err(fail(ProfileStage::NameValue));
    }
    let selector = Selector::parse("a[href]").map_err(|diagnostic_cause| {
        safe_diagnostics::error(
            "forum_profile",
            "build_identity_link_selector",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        fail(ProfileStage::MidLayout)
    })?;
    let mut matches = 0;
    for anchor in container.select(&selector) {
        let reference = anchor.attr("href").ok_or_else(|| fail(ProfileStage::MidLayout))?;
        let url = target.join(reference).map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "forum_profile",
                "resolve_identity_link",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            fail(ProfileStage::MidLayout)
        })?;
        let is_target_action = anchor.text().collect::<String>().trim()
            == "Send a Personal Message"
            || url.query_pairs().any(|(key, _)| key == "MID");
        if !is_target_action {
            continue;
        }
        if url.origin() != target.origin()
            || url.path() != "/index.php"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(fail(ProfileStage::MidLayout));
        }
        let mut pairs = BTreeMap::new();
        for (key, value) in url.query_pairs() {
            if !["s", "act", "CODE", "MID"].contains(&key.as_ref())
                || pairs.insert(key.into_owned(), value.into_owned()).is_some()
            {
                return Err(fail(ProfileStage::MidQuery));
            }
        }
        // 旧论坛 session query 只在当前内存剥离，永不作为后续请求参数或证据字段。
        pairs.remove("s");
        if pairs
            != BTreeMap::from([
                ("act".to_owned(), "Msg".to_owned()),
                ("CODE".to_owned(), "4".to_owned()),
                ("MID".to_owned(), uid.to_owned()),
            ])
        {
            return Err(fail(ProfileStage::MidBinding));
        }
        matches += 1;
    }
    if matches != 1 {
        // 数量只用于区分按钮缺失与目标歧义，不记录任何链接或页面内容。
        let error = InternalEvidenceError::Unproven;
        safe_diagnostics::warn(
            "forum_profile",
            "validate_mid_count",
            safe_diagnostics::project(&error),
            DiagnosticContext::default(),
        );
        return Err(error);
    }
    Ok(full_name)
}
