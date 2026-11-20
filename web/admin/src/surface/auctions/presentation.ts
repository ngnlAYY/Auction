// Admin 拍卖页面共享的状态、时间和文案映射。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { parseStrictRfc3339 } from "../../api/session-time";
import { type LocalTimeError, parseLocalDatetime } from "../../api/validation";

export function adminAuctionStatusLabel(status: string): string {
  switch (status) {
    case "draft":
      return t("admin-auctions-status-draft");
    case "preview":
      return t("admin-auctions-status-preview");
    case "live":
      return t("admin-auctions-status-live");
    case "final":
      return t("admin-auctions-status-final");
    case "settling":
      return t("admin-auctions-status-settling");
    case "settled":
      return t("admin-auctions-status-settled");
    case "cancelled":
      return t("admin-auctions-status-cancelled");
    default:
      return status;
  }
}

export function currentTimeMillis(): number {
  return Date.now();
}

export function scheduledTimeMillis(value: string): number | null {
  const milliseconds = Date.parse(value);
  return Number.isFinite(milliseconds) ? milliseconds : null;
}

/** 候选排期使用已经解析的 UTC，供表单校验及首次提交后原样重试。 */
export interface ScheduleTimes {
  readonly preview: string;
  readonly open: string;
  readonly end: string;
}

export type ScheduleField = "preview" | "open" | "end";

export type ScheduleIssue =
  | "required"
  | "invalid_time"
  | "nonexistent_local_time"
  | "ambiguous_local_time"
  | "preview_not_future"
  | "open_before_preview"
  | "open_not_future"
  | "end_not_after_open";

export function scheduleIssueMessage(issue: ScheduleIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "invalid_time":
      return t("admin-auctions-invalid-local-time");
    case "nonexistent_local_time":
      return t("admin-auctions-nonexistent-local-time");
    case "ambiguous_local_time":
      return t("admin-auctions-ambiguous-local-time");
    case "preview_not_future":
      return t("admin-auctions-create-field-preview-past");
    case "open_before_preview":
      return t("admin-auctions-create-field-open-before-preview");
    case "open_not_future":
      return t("admin-auctions-schedule-field-open-past");
    case "end_not_after_open":
      return t("admin-auctions-create-field-end-after-open");
    default:
      return unreachable(issue);
  }
}

/** 只在操作员提交时解析本地分钟。用户输入必须在浏览器时区内有唯一的 UTC 对应。 */
export function parseLocalScheduleTime(value: string): Result<string, ScheduleIssue> {
  if (value.length === 0) {
    return err("required");
  }
  if (utf8ByteLength(value) !== 16 || value[10] !== "T") {
    return err("invalid_time");
  }
  const parsed = parseLocalDatetime(value);
  if (parsed.ok) {
    return parsed;
  }
  return err(localTimeIssue(parsed.error));
}

function localTimeIssue(error: LocalTimeError): ScheduleIssue {
  switch (error) {
    case "invalid":
      return "invalid_time";
    case "nonexistent":
      return "nonexistent_local_time";
    case "ambiguous":
      return "ambiguous_local_time";
    default:
      return unreachable(error);
  }
}

export type ScheduleMode = "create" | "draft" | "preview";

/** 创建和改排期都要求三项严格递增。已预览的场次只检查开拍仍在未来。 */
export function validateSchedule(
  preview: Result<string, ScheduleIssue>,
  open: Result<string, ScheduleIssue>,
  end: Result<string, ScheduleIssue>,
  mode: ScheduleMode,
  now: number,
): Result<ScheduleTimes, readonly [ScheduleField, ScheduleIssue]> {
  if (!preview.ok) {
    return err(["preview", preview.error] as const);
  }
  const previewTime = preview.value;
  if (!open.ok) {
    return err(["open", open.error] as const);
  }
  const openTime = open.value;
  if (!end.ok) {
    return err(["end", end.error] as const);
  }
  const endTime = end.value;
  // 表单会原样保留未改动的预填时间，其小数秒可能精确到微秒；`Date.parse` 只到毫秒，
  // 会把严格递增的两个时刻判成相等。按完整纪元纳秒比较，浏览器时钟再换算到同一单位。
  const previewNanos = parseStrictRfc3339(previewTime);
  if (previewNanos === null) {
    return err(["preview", "invalid_time"] as const);
  }
  const openNanos = parseStrictRfc3339(openTime);
  if (openNanos === null) {
    return err(["open", "invalid_time"] as const);
  }
  const endNanos = parseStrictRfc3339(endTime);
  if (endNanos === null) {
    return err(["end", "invalid_time"] as const);
  }
  const nowNanos = BigInt(Math.trunc(now)) * 1_000_000n;
  if (
    (mode === "create" && previewNanos < nowNanos) ||
    (mode === "draft" && previewNanos <= nowNanos)
  ) {
    return err(["preview", "preview_not_future"] as const);
  }
  if (openNanos <= previewNanos) {
    return err(["open", "open_before_preview"] as const);
  }
  if (mode === "preview" && openNanos <= nowNanos) {
    return err(["open", "open_not_future"] as const);
  }
  if (endNanos <= openNanos) {
    return err(["end", "end_not_after_open"] as const);
  }
  return ok({ preview: previewTime, open: openTime, end: endTime });
}
