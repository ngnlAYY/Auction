//! 仅遍历当前发送账号的 Sent；分页来自实际链接，完整遍历后才判定唯一副本。

use std::collections::HashSet;
use std::collections::{BTreeMap, VecDeque};

use auction_application::forum::{ForumPmError, ForumPmMessage, ForumPmReceipt};
use scraper::ElementRef;
use scraper::Html;
use url::Url;

use super::protocol::{has_query, normalize, plain_body, profile_id, read_link};
use super::protocol::{pm_selector, sender_identity};
use super::{MAXIMUM_CANDIDATES, MAXIMUM_SCAN_BYTES, MAXIMUM_SCAN_PAGES};
use crate::clients::ehentai::client::EhentaiClient;
use crate::clients::ehentai::credentials::EhentaiCookie;
use crate::clients::ehentai::forms::valid_post_id;
use crate::safe_diagnostics;

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
                    // PM 回读有独立的固定页数预算，与主题扫描的 maximum_topic_pages 无关。
                    if discovered.len() > MAXIMUM_SCAN_PAGES {
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
    let links = pm_selector!("a[href]")?;
    let mut pages = HashSet::new();
    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    for link in document.select(links) {
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
            for profile in row.select(links) {
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
            let Some(url) = sent_page_link(target, reference)? else {
                continue;
            };
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

/// 把 Sent 列表链接规范为要跟随的真实分页；第 0 页（含各排序变体）返回 `None`。
///
/// 表头排序链接以空 `st=` 指向第 0 页的另一种排序，侧栏入口也是第 0 页；按默认排序
/// 读完全部 `st>0` 分页即为完整遍历，排序变体只会重复读取同一批邮件。字段格式仍严格
/// 校验，规范化时去掉 `sort` 并把 `st` 写成规范十进制，保证同一页只入队一次。
fn sent_page_link(base: &Url, reference: &str) -> Result<Option<Url>, ForumPmError> {
    let (mut url, mut pairs) = read_link(base, reference, &["act", "CODE", "VID", "sort", "st"])?;
    require_sent(&pairs, "01")?;
    if pairs.get("sort").is_some_and(|v| {
        v.is_empty()
            || v.len() > 32
            || !v.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    }) {
        return Err(ForumPmError::Protocol);
    }
    let offset = match pairs.get("st").map(String::as_str) {
        None | Some("") => 0,
        Some(value) if value.bytes().all(|b| b.is_ascii_digit()) => value.parse::<u64>().map_err(
            safe_diagnostics::warn_map("forum_pm", "parse_sent_page_offset", || {
                ForumPmError::Protocol
            }),
        )?,
        Some(_) => return Err(ForumPmError::Protocol),
    };
    if offset == 0 {
        return Ok(None);
    }
    pairs.remove("sort");
    pairs.insert("st".to_owned(), offset.to_string());
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs.iter());
    Ok(Some(url))
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
    let expected_ids = target
        .query_pairs()
        .filter(|(key, _)| key == "MSID")
        .map(|(_, value)| value.into_owned())
        .collect::<Vec<_>>();
    if expected_ids.len() != 1 || !valid_post_id(&expected_ids[0]) {
        return Err(ForumPmError::Protocol);
    }
    let identity_selector = pm_selector!("input[name='skinurlbits']")?;
    let mut identities = document.select(identity_selector);
    let identity = identities.next().ok_or(ForumPmError::Protocol)?;
    if identities.next().is_some() {
        return Err(ForumPmError::Protocol);
    }
    let reference = identity.attr("value").ok_or(ForumPmError::Protocol)?;
    let (_, pairs) = read_link(
        target,
        &format!("index.php?{reference}"),
        &["act", "CODE", "VID", "MSID", "cal_id"],
    )?;
    require_sent(&pairs, "03")?;
    if pairs.get("MSID") != expected_ids.first() || sender_identity(&document, target)? != sender {
        return Err(ForumPmError::Protocol);
    }
    let body_selector = pm_selector!(".postcolor")?;
    let mut bodies = document.select(body_selector);
    let body = bodies.next().ok_or(ForumPmError::Protocol)?;
    if bodies.next().is_some() || plain_body(body)? != normalize(&message.body) {
        return Err(ForumPmError::Protocol);
    }
    let titles = pm_selector!(".maintitle, .pformstrip, .formsubtitle, .tableheader, strong, b")?;
    let exact_title = document.select(titles).any(|node| {
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
    use scraper::Html;
    use url::Url;

    use super::{
        ForumPmMessage, is_sent_list_link, parse_list, read_link, sender_identity, verify_detail,
    };

    const LIST: &str = include_str!("../../../../../tests/fixtures/forum/pm-sent-list.html");
    const DETAIL: &str = include_str!("../../../../../tests/fixtures/forum/pm-detail-1558026.html");

    fn message() -> ForumPmMessage {
        ForumPmMessage {
            notification_id: uuid::Uuid::nil(),
            recipient_forum_account_id: "100".into(),
            title: "夹具私信主题".into(),
            body: "fixture frozen body".into(),
        }
    }

    #[test]
    fn captured_sent_list_accepts_empty_first_page_offset() {
        let target =
            Url::parse("https://forums.e-hentai.org/index.php?act=Msg&CODE=01&VID=sent").unwrap();
        let document = Html::parse_document(LIST);
        assert_eq!(sender_identity(&document, &target).unwrap(), "1000001");
        for link in document.select(&scraper::Selector::parse("a[href]").unwrap()) {
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
        // 表头排序链接与侧栏入口都是第 0 页的变体，不作为分页跟随。
        assert!(page.pages.is_empty());
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
    fn real_pagination_is_followed_once_without_sort_variants() {
        let target =
            Url::parse("https://forums.e-hentai.org/index.php?act=Msg&CODE=01&VID=sent").unwrap();
        // 两个指向同一页的分页链接（带与不带排序、前导零）只入队一次，且去掉排序参数。
        let pagination = concat!(
            "<a href=\"https://forums.e-hentai.org/index.php?act=Msg&amp;CODE=01&amp;VID=sent",
            "&amp;sort=msg_date&amp;st=50\">2</a>",
            "<a href=\"https://forums.e-hentai.org/index.php?act=Msg&amp;CODE=01&amp;VID=sent",
            "&amp;st=050\">2</a>",
            "<a href=\"https://forums.e-hentai.org/index.php?act=Msg&amp;CODE=01&amp;VID=sent",
            "&amp;st=0\">1</a>",
        );
        let list =
            LIST.replace("<!-- INBOX TABLE -->", &format!("{pagination}<!-- INBOX TABLE -->"));
        let page = parse_list(&list, &target, &message(), "1000001").unwrap();
        let expected =
            Url::parse("https://forums.e-hentai.org/index.php?CODE=01&VID=sent&act=Msg&st=50")
                .unwrap();
        assert_eq!(page.pages, vec![expected]);
    }

    #[test]
    fn captured_detail_title_binds_to_formsubtitle() {
        let target = Url::parse(
            "https://forums.e-hentai.org/index.php?act=Msg&CODE=03&VID=sent&MSID=1558026",
        )
        .unwrap();
        let document = Html::parse_document(DETAIL);
        let body =
            document.select(&scraper::Selector::parse(".postcolor").unwrap()).next().unwrap();
        let source = document.root_element().html().replace(&body.inner_html(), &message().body);
        assert!(verify_detail(&source, &target, &message(), "1000001").is_ok());
        assert!(
            verify_detail(
                &source.replace("夹具私信主题", "wrong title"),
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
