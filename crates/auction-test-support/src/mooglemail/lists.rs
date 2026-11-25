//! 列表页的有状态改写：按替身状态移除行，或把已采集的末页改成真实的末页分页结构。

use scraper::{Html as Document, Selector};

use super::fixtures;

/// 把指向未采集页的 Next 链接替换为真实末页的禁用结构；没有链接时原样返回。
pub(super) fn end_pagination(html: String) -> String {
    const NEXT: &str = ">Next &gt;</a>";
    let Some(end) = html.find(NEXT) else { return html };
    let Some(start) = html[..end].rfind("<a ") else { return html };
    format!("{}<span>Next &gt;</span>{}", &html[..start], &html[end + NEXT.len()..])
}

pub(super) fn list_overlay(source: &str, keep: impl Fn(&str) -> bool) -> String {
    let document = Document::parse_document(source);
    let selector = Selector::parse("table#mmail_list tr[onclick]").expect("constant selector");
    let mut html = document.root_element().html();
    let mut remaining = 0;
    for row in document.select(&selector) {
        let onclick = row.attr("onclick").unwrap_or_default();
        let mid = onclick
            .split("mid=")
            .nth(1)
            .unwrap_or_default()
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .unwrap_or_default();
        if keep(mid) {
            remaining += 1;
        } else {
            html = html.replace(&row.html(), "");
        }
    }
    if remaining == 0 {
        // 空集合仍使用真实 Inbox 的空行结构，避免把无行表格误当协议漂移。
        let empty = Document::parse_document(
            fixtures::page("main", "inbox", 0, None).expect("captured inbox"),
        );
        let empty_selector = Selector::parse("#mmail_nnm").expect("constant selector");
        let empty_row = empty
            .select(&empty_selector)
            .next()
            .and_then(|cell| cell.parent())
            .and_then(scraper::ElementRef::wrap)
            .expect("captured empty row");
        let table_selector = Selector::parse("table#mmail_list").expect("constant selector");
        let rendered_table = Document::parse_fragment(&html);
        let current =
            rendered_table.select(&table_selector).next().expect("rendered mail table").html();
        html = html.replace(
            &current,
            &current.replace("</tbody>", &format!("{}</tbody>", empty_row.html())),
        );
    }
    html
}
