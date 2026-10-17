//! 论坛目标资料联合证明：固定 UID GET、资料布局及目标 MID 一致，不读取页头登录身份。

use std::collections::BTreeMap;

use auction_application::{
    ForumTargetProfileEvidence, InternalEvidenceError, valid_external_positive_id,
    valid_mail_player_name,
};
use scraper::{ElementRef, Node};
use sha2::{Digest, Sha256};

use super::*;

impl EhentaiClient {
    pub(in crate::clients::ehentai) async fn read_target_profile(
        &self,
        target_uid: &str,
        cookie: &EhentaiCookie,
    ) -> Result<ForumTargetProfileEvidence, InternalEvidenceError> {
        self.validate_pm_client().map_err(|_| InternalEvidenceError::Unavailable)?;
        if !valid_external_positive_id(target_uid) {
            return Err(InternalEvidenceError::InvalidRequest);
        }
        let target = self
            .index_url(&[("showuser", target_uid)], EhentaiOperation::CheckSession)
            .map_err(|_| InternalEvidenceError::InvalidRequest)?;
        // 仅 GET 资料；复用强制不重定向的安全池，不经过发帖或 PM 准备流程。
        let source = self
            .pm_get(target.clone(), cookie)
            .await
            .map_err(|_| InternalEvidenceError::Unavailable)?;
        let name = parse_target_profile(&source, &target, target_uid)?;
        self.check_ownership().await.map_err(|_| InternalEvidenceError::Revoked)?;
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
    let selector = Selector::parse(query).map_err(|_| InternalEvidenceError::Unproven)?;
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
    let fail = InternalEvidenceError::Unproven;
    let document = Html::parse_document(source);
    let name = select_one(&document, "#profilename")?;
    let options = select_one(&document, "#profile-options")?;
    let name_frame = borderwrap(name).ok_or(fail)?;
    if borderwrap(options).map(|node| node.id()) != Some(name_frame.id()) {
        return Err(fail);
    }
    let container = name_frame.parent().and_then(ElementRef::wrap).ok_or(fail)?;
    if container.value().name() != "div"
        || container.attr("class").is_some()
        || container.parent().and_then(ElementRef::wrap).is_none_or(|node| {
            node.value().name() != "div" || !node.value().classes().any(|class| class == "page")
        })
    {
        return Err(fail);
    }
    // 只接受已观察的 FONT/文本名称；不把隐藏脚本、图像替代文字当显示名。
    for node in name.descendants().skip(1) {
        match node.value() {
            Node::Text(_) => {}
            Node::Element(value)
                if value.name() == "font"
                    && value.attr("style").is_none()
                    && value.attr("hidden").is_none() => {}
            _ => return Err(fail),
        }
    }
    let full_name = name.text().collect::<String>().trim().to_owned();
    if !valid_mail_player_name(&full_name) {
        return Err(fail);
    }
    let selector = Selector::parse("a[href]").map_err(|_| fail)?;
    let mut matches = 0;
    for anchor in container.select(&selector) {
        let reference = anchor.attr("href").ok_or(fail)?;
        let url = target.join(reference).map_err(|_| fail)?;
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
            return Err(fail);
        }
        let mut pairs = BTreeMap::new();
        for (key, value) in url.query_pairs() {
            if !["s", "act", "CODE", "MID"].contains(&key.as_ref())
                || pairs.insert(key.into_owned(), value.into_owned()).is_some()
            {
                return Err(fail);
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
            return Err(fail);
        }
        matches += 1;
    }
    if matches != 1 {
        return Err(fail);
    }
    Ok(full_name)
}
