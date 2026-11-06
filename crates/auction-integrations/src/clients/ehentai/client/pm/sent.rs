//! 仅遍历当前发送账号的 Sent；分页来自实际链接，完整遍历后才判定唯一副本。

use std::collections::{BTreeMap, VecDeque};

use scraper::ElementRef;

use super::*;
use protocol::{has_query, normalize, plain_body, profile_id, read_link};

struct Candidate {
    target: Url,
    recipient: String,
}

struct SentPage {
    pages: Vec<Url>,
    candidates: BTreeMap<String, Candidate>,
}

impl EhentaiClient {
    pub(super) async fn scan_sent(
        &self,
        message: &ForumPmMessage,
        sender: &str,
        cookie: &EhentaiCookie,
        metrics_world: auction_application::external_metrics::ForumWorld,
    ) -> Result<Option<ForumPmReceipt>, ForumPmError> {
        let initial = self.pm_url(&[("act", "Msg"), ("CODE", "01"), ("VID", "sent")])?;
        let (initial, _) = read_link(&initial, initial.as_str(), &["act", "CODE", "VID"])?;
        let mut pages = VecDeque::from([initial.clone()]);
        let mut discovered = HashSet::from([initial]);
        let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
        let mut total_bytes = 0_usize;
        while let Some(target) = pages.pop_front() {
            let source = self
                .scan_get(
                    target.clone(),
                    cookie,
                    total_bytes,
                    metrics_world,
                    auction_application::external_metrics::ForumOperation::PmSentList,
                )
                .await?;
            add_bytes(&mut total_bytes, source.len())?;
            let page = parse_list(&source, &target, message, sender)?;
            for next in page.pages {
                if discovered.insert(next.clone()) {
                    if discovered.len() > MAXIMUM_SCAN_PAGES.min(self.maximum_topic_pages) {
                        return Err(ForumPmError::Protocol);
                    }
                    pages.push_back(next);
                }
            }
            for (msid, candidate) in page.candidates {
                if let Some(previous) = candidates.get(&msid) {
                    if previous.recipient != candidate.recipient
                        || previous.target != candidate.target
                    {
                        return Err(ForumPmError::Protocol);
                    }
                } else {
                    candidates.insert(msid, candidate);
                }
                if candidates.len() > MAXIMUM_CANDIDATES {
                    return Err(ForumPmError::Protocol);
                }
            }
        }
        let mut receipt = None;
        // 不在首个匹配处返回；所有已发现同标题候选都必须证明，重复或不确定一律不成功。
        for (msid, candidate) in candidates {
            if candidate.recipient != message.recipient_forum_account_id {
                return Err(ForumPmError::Protocol);
            }
            let source = self
                .scan_get(
                    candidate.target.clone(),
                    cookie,
                    total_bytes,
                    metrics_world,
                    auction_application::external_metrics::ForumOperation::PmSentDetail,
                )
                .await?;
            add_bytes(&mut total_bytes, source.len())?;
            verify_detail(&source, &candidate.target, message, sender)?;
            if receipt.is_some() {
                return Err(ForumPmError::Protocol);
            }
            receipt = Some(ForumPmReceipt {
                message: message.clone(),
                sender_forum_account_id: sender.to_owned(),
                sent_message_id: msid,
            });
        }
        // None 只表示本轮有界读取未找到证据；不表示未发送，更不能授权重寄。
        Ok(receipt)
    }

    async fn scan_get(
        &self,
        target: Url,
        cookie: &EhentaiCookie,
        consumed: usize,
        metrics_world: auction_application::external_metrics::ForumWorld,
        metrics_operation: auction_application::external_metrics::ForumOperation,
    ) -> Result<String, ForumPmError> {
        let remaining = MAXIMUM_SCAN_BYTES
            .checked_sub(consumed)
            .filter(|remaining| *remaining > 0)
            .ok_or(ForumPmError::Protocol)?;
        // 读取前收紧流式字节门禁，避免最后一页仍按完整单页上限消费聚合预算。
        let mut reader = self.clone();
        reader.maximum_response_bytes = self.maximum_response_bytes.min(remaining);
        reader.pm_get(target, cookie, metrics_world, metrics_operation).await
    }
}

fn add_bytes(total: &mut usize, count: usize) -> Result<(), ForumPmError> {
    *total = total
        .checked_add(count)
        .filter(|v| *v <= MAXIMUM_SCAN_BYTES)
        .ok_or(ForumPmError::Protocol)?;
    Ok(())
}

fn parse_list(
    source: &str,
    target: &Url,
    message: &ForumPmMessage,
    sender: &str,
) -> Result<SentPage, ForumPmError> {
    let document = Html::parse_document(source);
    if sender_identity(&document, target)? != sender {
        // 换账号只阻断旧 sender 对账，不把当前仍有效的 Cookie 标为失效。
        return Err(ForumPmError::Protocol);
    }
    let links = selector("a[href]")?;
    let mut pages = HashSet::new();
    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    for link in document.select(&links) {
        let reference = link.attr("href").ok_or(ForumPmError::Protocol)?;
        let exact_title = link.text().collect::<String>().trim() == message.title;
        if exact_title {
            let (url, pairs) = read_link(target, reference, &["act", "CODE", "VID", "MSID"])?;
            require_sent(&pairs, "03")?;
            let msid = pairs
                .get("MSID")
                .filter(|v| valid_post_id(v))
                .ok_or(ForumPmError::Protocol)?
                .clone();
            let row = link
                .ancestors()
                .filter_map(ElementRef::wrap)
                .find(|v| v.value().name() == "tr")
                .ok_or(ForumPmError::Protocol)?;
            let mut recipients = HashSet::new();
            for profile in row.select(&links) {
                let href = profile.attr("href").ok_or(ForumPmError::Protocol)?;
                if has_query(target, href, "showuser") {
                    recipients.insert(profile_id(target, href)?);
                }
            }
            if recipients.len() != 1 {
                return Err(ForumPmError::Protocol);
            }
            let recipient = recipients.into_iter().next().ok_or(ForumPmError::Protocol)?;
            if let Some(previous) = candidates.get(&msid)
                && (previous.recipient != recipient || previous.target != url)
            {
                return Err(ForumPmError::Protocol);
            }
            candidates.insert(msid, Candidate { target: url, recipient });
            if candidates.len() > MAXIMUM_CANDIDATES {
                return Err(ForumPmError::Protocol);
            }
        } else if is_sent_list_link(target, reference) {
            let (url, pairs) = read_link(target, reference, &["act", "CODE", "VID", "sort", "st"])?;
            require_sent(&pairs, "01")?;
            // 旧版 IPB 排序链接以空 st= 表示第一页；其余 URL/账号门禁保持严格。
            if pairs.get("st").is_some_and(|v| {
                !v.is_empty()
                    && (!v.bytes().all(|b| b.is_ascii_digit()) || v.parse::<u64>().is_err())
            }) || pairs.get("sort").is_some_and(|v| {
                v.is_empty()
                    || v.len() > 32
                    || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            }) {
                return Err(ForumPmError::Protocol);
            }
            pages.insert(url);
            if pages.len() > MAXIMUM_SCAN_PAGES {
                return Err(ForumPmError::Protocol);
            }
        }
    }
    let mut pages = pages.into_iter().collect::<Vec<_>>();
    pages.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(SentPage { pages, candidates })
}

fn is_sent_list_link(base: &Url, reference: &str) -> bool {
    // href="#" 是窗口/工具栏控件，不因继承当前查询而变成 Sent 分页。
    if reference.is_empty() || reference.starts_with('#') {
        return false;
    }
    base.join(reference).is_ok_and(|url| {
        url.query_pairs().any(|(k, v)| k == "VID" && v == "sent")
            && url.query_pairs().any(|(k, v)| k == "CODE" && v == "01")
    })
}

fn require_sent(pairs: &BTreeMap<String, String>, code: &str) -> Result<(), ForumPmError> {
    if pairs.get("act").map(String::as_str) != Some("Msg")
        || pairs.get("CODE").map(String::as_str) != Some(code)
        || pairs.get("VID").map(String::as_str) != Some("sent")
    {
        return Err(ForumPmError::Protocol);
    }
    Ok(())
}

fn verify_detail(
    source: &str,
    target: &Url,
    message: &ForumPmMessage,
    sender: &str,
) -> Result<(), ForumPmError> {
    let document = Html::parse_document(source);
    if sender_identity(&document, target)? != sender {
        return Err(ForumPmError::Protocol);
    }
    let body_selector = selector(".postcolor")?;
    let mut bodies = document.select(&body_selector);
    let body = bodies.next().ok_or(ForumPmError::Protocol)?;
    if bodies.next().is_some() || plain_body(body)? != normalize(&message.body) {
        return Err(ForumPmError::Protocol);
    }
    let titles = selector(".maintitle, .pformstrip, .formsubtitle, .tableheader, strong, b")?;
    let exact_title = document.select(&titles).any(|node| {
        !node
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| parent.value().classes().any(|class| class == "postcolor"))
            && node.text().collect::<String>().trim() == message.title
    });
    if !exact_title {
        return Err(ForumPmError::Protocol);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ForumPmMessage, is_sent_list_link, parse_list, read_link, selector, sender_identity,
        verify_detail,
    };
    use scraper::Html;
    use url::Url;

    const LIST: &str = include_str!("../../../../../tests/fixtures/w6_3/forum/pm-sent-list.html");
    const DETAIL: &str =
        include_str!("../../../../../tests/fixtures/w6_3/forum/pm-detail-1558026.html");

    fn message() -> ForumPmMessage {
        ForumPmMessage {
            notification_id: uuid::Uuid::nil(),
            recipient_forum_account_id: "100".into(),
            title: "请求增加H@H服务器".into(),
            body: "fixture frozen body".into(),
        }
    }

    #[test]
    fn captured_sent_list_accepts_empty_first_page_offset() {
        let target =
            Url::parse("https://forums.e-hentai.org/index.php?act=Msg&CODE=01&VID=sent").unwrap();
        let document = Html::parse_document(LIST);
        assert_eq!(sender_identity(&document, &target).unwrap(), "1000001");
        for link in document.select(&selector("a[href]").unwrap()) {
            let href = link.attr("href").unwrap();
            if is_sent_list_link(&target, href) {
                assert!(
                    read_link(&target, href, &["act", "CODE", "VID", "sort", "st"]).is_ok(),
                    "{href}"
                );
            }
        }
        let page = parse_list(LIST, &target, &message(), "1000001").unwrap();
        assert_eq!(page.candidates.len(), 1);
        assert!(page.candidates.contains_key("1558026"));
        for replacement in ["st=-1", "st=bad", "st=18446744073709551616", "st=&amp;st=0"] {
            assert!(
                parse_list(
                    &LIST.replace("st=\"", &format!("{replacement}\"")),
                    &target,
                    &message(),
                    "1000001"
                )
                .is_err()
            );
        }
        for replacement in ["showuser=999", "showuser=1000001&amp;showuser=1000001"] {
            assert!(
                parse_list(
                    &LIST.replace("showuser=1000001", replacement),
                    &target,
                    &message(),
                    "1000001"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn captured_detail_title_binds_to_formsubtitle() {
        let target = Url::parse(
            "https://forums.e-hentai.org/index.php?act=Msg&CODE=03&VID=sent&MSID=1558026",
        )
        .unwrap();
        let document = Html::parse_document(DETAIL);
        let body = document.select(&selector(".postcolor").unwrap()).next().unwrap();
        let source = document.root_element().html().replace(&body.inner_html(), &message().body);
        assert!(verify_detail(&source, &target, &message(), "1000001").is_ok());
        assert!(
            verify_detail(
                &source.replace("请求增加H@H服务器", "wrong title"),
                &target,
                &message(),
                "1000001"
            )
            .is_err()
        );
        assert!(
            verify_detail(
                &source.replace("fixture frozen body", "wrong body"),
                &target,
                &message(),
                "1000001"
            )
            .is_err()
        );
        assert!(verify_detail(&source, &target, &message(), "999").is_err());
    }
}
