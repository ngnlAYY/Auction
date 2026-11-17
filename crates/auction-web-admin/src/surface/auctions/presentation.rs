//! Admin 拍卖页面共享的状态、时间和文案映射。

use auction_web_shared::t;

pub(super) fn admin_auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("admin-auctions-status-draft"),
        "preview" => t!("admin-auctions-status-preview"),
        "live" => t!("admin-auctions-status-live"),
        "final" => t!("admin-auctions-status-final"),
        "settling" => t!("admin-auctions-status-settling"),
        "settled" => t!("admin-auctions-status-settled"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
}

pub(super) fn current_time_millis() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::new_0().get_time() as i64
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        auction_time::Timestamp::now().timestamp_millis()
    }
}

pub(super) fn scheduled_time_millis(value: &str) -> Option<i64> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        let milliseconds = date.get_time();
        milliseconds.is_finite().then_some(milliseconds as i64)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        auction_time::Timestamp::parse_from_rfc3339(value)
            .ok()
            .map(auction_time::Timestamp::timestamp_millis)
    }
}

/// 创建拍卖的计划时间校验未通过的具体原因，用于把错误定位到对应字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuctionScheduleIssue {
    /// 任一时间为空或无法解析（datetime-local 正常不会产出，但防御畸形值）。
    Unparseable,
    /// 预览时间不晚于现在。
    PreviewNotFuture,
    /// 开放时间早于预览时间。
    OpenBeforePreview,
    /// 结束时间不晚于开放时间。
    EndNotAfterOpen,
}

/// 校验创建拍卖的三项计划时间，返回首个未通过的约束；全部通过返回 None。
pub(super) fn auction_schedule_issue(
    preview: &str,
    open: &str,
    end: &str,
) -> Option<AuctionScheduleIssue> {
    #[cfg(target_arch = "wasm32")]
    {
        let (Some(preview), Some(open), Some(end)) = (
            local_datetime_millis(preview),
            local_datetime_millis(open),
            local_datetime_millis(end),
        ) else {
            return Some(AuctionScheduleIssue::Unparseable);
        };
        if preview < js_sys::Date::new_0().get_time() {
            return Some(AuctionScheduleIssue::PreviewNotFuture);
        }
        if open < preview {
            return Some(AuctionScheduleIssue::OpenBeforePreview);
        }
        if end <= open {
            return Some(AuctionScheduleIssue::EndNotAfterOpen);
        }
        None
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // 宿主构建没有浏览器时区，`datetime-local` 的分钟文本直接按 UTC 解释。
        let parse =
            |value: &str| auction_time::Timestamp::parse_from_rfc3339(&format!("{value}:00Z")).ok();
        let (Some(preview), Some(open), Some(end)) = (parse(preview), parse(open), parse(end))
        else {
            return Some(AuctionScheduleIssue::Unparseable);
        };
        if preview < auction_time::Timestamp::now() {
            return Some(AuctionScheduleIssue::PreviewNotFuture);
        }
        if open < preview {
            return Some(AuctionScheduleIssue::OpenBeforePreview);
        }
        if end <= open {
            return Some(AuctionScheduleIssue::EndNotAfterOpen);
        }
        None
    }
}

/// 校验计划修改输入：Draft 三项都在未来；Preview 保留既有预览时间并要求开拍仍在未来。
pub(super) fn valid_schedule_update(status: &str, preview: &str, open: &str, end: &str) -> bool {
    // RFC3339 保留秒与小数秒；只有编辑过的本地值需要按浏览器时区规范化。
    let parse = |value: &str| {
        auction_time::Timestamp::parse_from_rfc3339(&crate::api::normalize_datetime(value)).ok()
    };
    let (Some(preview), Some(open), Some(end), Some(now)) = (
        parse(preview),
        parse(open),
        parse(end),
        auction_time::Timestamp::from_timestamp_millis(current_time_millis()),
    ) else {
        return false;
    };
    preview < open
        && open < end
        && match status {
            "draft" => preview > now,
            "preview" => open > now,
            _ => false,
        }
}

#[cfg(target_arch = "wasm32")]
fn local_datetime_millis(value: &str) -> Option<f64> {
    // `datetime-local` 按浏览器本地时区解释；这里的时间戳必须与提交时的 UTC 转换使用同一
    // 规则，否则用户在非 UTC 时区创建拍卖时，前端校验与服务端看到的时间会产生偏差。
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    let milliseconds = date.get_time();
    milliseconds.is_finite().then_some(milliseconds)
}
