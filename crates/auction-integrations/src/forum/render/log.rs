//! 系统更新楼的安全、限长诊断展示；错误数量不能阻断后续价格镜像与处理水位。

use std::fmt::Write;

use auction_application::forum::{
    ForumRenderError, ForumRenderedPost, ForumSystemLogEntry, MAX_FORUM_CATALOG_PAGES,
    forum_system_log_marker,
};
use auction_domain::AuctionId;

use super::{MAX_BODY_BYTES, PRODUCTION_ENDPOINT, bbcode_text, valid_forum_post_id, valid_text};

const MAX_DISPLAYED_ENTRIES: usize = 100;
const FOOTER_RESERVE_BYTES: usize = 2048;

/// 相同事实稳定渲染；先展示阻塞进度的 pending，再展示最近的拒绝/格式错误及忽略记录。
pub(super) fn render_system_log(
    auction_id: AuctionId,
    catalog_page_count: usize,
    processed_through_post_number: usize,
    mut entries: Vec<ForumSystemLogEntry>,
) -> Result<ForumRenderedPost, ForumRenderError> {
    if auction_id.is_nil()
        || !(1..=MAX_FORUM_CATALOG_PAGES).contains(&catalog_page_count)
        || processed_through_post_number <= catalog_page_count
        || entries.len() > 20_000
    {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    let system_post_number = catalog_page_count + 1;
    if entries.iter().any(|entry| {
        entry.post_number == 0
            || entry.post_number == system_post_number
            || !valid_forum_post_id(&entry.post_id)
            || entry.line_number.is_some_and(|line| line == 0)
            || !valid_text(&entry.code, 128)
            || !valid_text(&entry.message, 512)
    }) {
        return Err(ForumRenderError::InvalidSnapshot);
    }
    entries.sort_by(|left, right| {
        left.post_number
            .cmp(&right.post_number)
            .then_with(|| left.post_id.cmp(&right.post_id))
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
            .then_with(|| left.ignored.cmp(&right.ignored))
    });
    entries.dedup();
    let pending_count = entries.iter().filter(|entry| is_pending(entry)).count();
    let ignored_count = entries.iter().filter(|entry| entry.ignored).count();
    let warning_count = entries.len() - pending_count - ignored_count;
    let mut body = String::new();
    writeln!(
        &mut body,
        "Updated through [b][size=5]#{processed_through_post_number}[/size][/b] — \
         [b]{pending_count}[/b] pending, [b]{warning_count}[/b] warnings, [b]{ignored_count}[/b] ignored."
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    let catalog_label = if catalog_page_count == 1 {
        "post #1".to_owned()
    } else {
        format!("posts #1-#{catalog_page_count}")
    };
    writeln!(
        &mut body,
        "[size=1][color=#666666]Accepted bids and current prices appear in {catalog_label}. \
         Bids begin at post #{}. Progress means processed, not necessarily accepted. \
         Check the auction website for authoritative results.[/color][/size]",
        system_post_number + 1
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    let mut displayed = 0;
    if pending_count > 0 {
        body.push_str(
            "\n[center][b][size=3]Pending — result not confirmed[/size][/b][/center]\n\n",
        );
        append_entries(
            &mut body,
            entries.iter().filter(|entry| is_pending(entry)),
            &mut displayed,
        )?;
    }
    body.push_str(
        "\n[center][b][color=#B00020][size=3]Warning Log[/size][/color][/b][/center]\n\n",
    );
    append_entries(
        &mut body,
        entries.iter().rev().filter(|entry| !entry.ignored && !is_pending(entry)),
        &mut displayed,
    )?;
    if warning_count == 0 {
        body.push_str("[color=#666666]No warnings.[/color]\n");
    }
    if ignored_count > 0 {
        body.push_str("\n[center][b][size=3]Ignored Posts[/size][/b][/center]\n\n");
        append_entries(
            &mut body,
            entries.iter().rev().filter(|entry| entry.ignored),
            &mut displayed,
        )?;
    }
    if displayed < entries.len() {
        writeln!(
            &mut body,
            "\n[size=1]Showing {displayed} of {} diagnostics to fit the forum post limit. \
             Pending items take priority, followed by recent warnings. Omitted entries are not resolved by omission.[/size]",
            entries.len()
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
    }
    body.push_str(
        "\n[size=1][color=#666666][i]For format errors, post a corrected new reply; do not edit the original. \
         Pending commands retry automatically; do not submit duplicates. \
         Maintained by the auction worker. Do not place bids in this post.[/i][/color][/size]\n",
    );
    writeln!(
        &mut body,
        "[size=1][color=transparent]{}[/color][/size]",
        forum_system_log_marker(auction_id)
    )
    .map_err(|_| ForumRenderError::BodyTooLarge)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ForumRenderError::BodyTooLarge);
    }
    Ok(ForumRenderedPost { body })
}

fn is_pending(entry: &ForumSystemLogEntry) -> bool {
    !entry.ignored
        && matches!(
            entry.code.as_str(),
            "command_pending" | "command_processing_pending" | "identity_processing_pending"
        )
}

fn append_entries<'a>(
    body: &mut String,
    entries: impl Iterator<Item = &'a ForumSystemLogEntry>,
    displayed: &mut usize,
) -> Result<(), ForumRenderError> {
    for entry in entries {
        if *displayed == MAX_DISPLAYED_ENTRIES {
            break;
        }
        let mut line = String::new();
        write!(
            &mut line,
            "[url={PRODUCTION_ENDPOINT}/index.php?act=findpost&pid={}][b]#{}[/b][/url] [color=#777777]",
            entry.post_id, entry.post_number
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        if let Some(line_number) = entry.line_number {
            write!(&mut line, "line {line_number} · ")
                .map_err(|_| ForumRenderError::BodyTooLarge)?;
        }
        writeln!(
            &mut line,
            "{}[/color] — {}",
            bbcode_text(&entry.code),
            bbcode_text(&entry.message)
        )
        .map_err(|_| ForumRenderError::BodyTooLarge)?;
        // BBCode 转义可能增长正文；按最终 UTF-8 字节计量，不截断标签或单条错误。
        if body.len() + line.len() > MAX_BODY_BYTES - FOOTER_RESERVE_BYTES {
            break;
        }
        body.push_str(&line);
        *displayed += 1;
    }
    Ok(())
}
