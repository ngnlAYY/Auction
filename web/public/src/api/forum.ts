// Public 论坛镜像 HTTP DTO 与请求。

import {
  type PublicForumResponse,
  PublicForumResponseSchema,
  PublicForumSyncStatusResponseSchema,
} from "@auction/contracts/dto/public/catalog/forum";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { validUuid } from "@auction/web-shared/ui/validation";
import { sendGet } from "./clock";
import { decodeErrorCode } from "./decoder";
import { parseStrictRfc3339 } from "./session";

/** Public 论坛镜像线程投影。 */
export type ForumThread = PublicForumResponse;

/** 读取开关；开关只来自 Public 的安全投影，请求或解码失败不能当作关闭。 */
export async function fetchForumSyncEnabled(signal: AbortSignal): Promise<Result<boolean, string>> {
  const sent = await sendGet("/api/v1/forum/sync-status", signal);
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  // `JSON.parse` 无法识别重复键，重复键按最后一个值处理；后端不输出重复键。源实现直接反序列化，
  // 失败时返回解析器错误文本；页面只判断 `Err(_)`，这里统一为 invalid_response。
  const payload = response.json();
  if (!payload.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  const decoded = decode(apiResponseSchema(PublicForumSyncStatusResponseSchema), payload.value);
  if (!decoded.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  return ok(decoded.value.data.enabled);
}

/** 读取单场拍卖的论坛镜像线程，并校验会直接进入页面的内容边界。 */
export async function fetchForumThread(
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<ForumThread, string>> {
  const url = `/api/v1/auctions/${auctionId}/forum`;
  const sent = await sendGet(url, signal);
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  // `JSON.parse` 无法识别重复键，重复键按最后一个值处理；后端不输出重复键。源实现直接反序列化，
  // 失败时返回解析器错误文本；页面只判断 `Err(_)`，这里统一为 invalid_response。
  const payload = response.json();
  if (!payload.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  const decoded = decode(apiResponseSchema(PublicForumResponseSchema), payload.value);
  if (!decoded.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  if (!validForumThread(decoded.value.data, auctionId)) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  return ok(decoded.value.data);
}

/** 恢复旧浏览器的严格语义边界，避免形状正确但内容损坏的论坛投影进入页面。 */
function validForumThread(thread: ForumThread, auctionId: string): boolean {
  return (
    thread.auction_id === auctionId &&
    validUuid(thread.auction_id) &&
    thread.source_revision >= 0 &&
    thread.content_revision >= 0 &&
    thread.processed_through_post_number >= 2 &&
    validStrictRfc3339(thread.updated_at) &&
    validOptionalTimestamp(thread.content_synced_at) &&
    validOptionalTimestamp(thread.content_last_success_at) &&
    validOptionalTimestamp(thread.content_last_error_at) &&
    (thread.content_last_error_at != null) === (thread.content_last_error_code != null) &&
    (thread.content_last_error_code == null || validErrorCode(thread.content_last_error_code)) &&
    (thread.status === "failed" ||
      thread.status === "pending" ||
      thread.status === "published" ||
      thread.status === "publishing") &&
    (thread.status !== "published" ||
      (thread.content_revision > 0 &&
        thread.content_synced_at != null &&
        thread.thread_url != null &&
        validForumUrl(thread.thread_url) &&
        thread.title != null &&
        thread.title !== "" &&
        thread.body != null &&
        thread.body !== ""))
  );
}

function validOptionalTimestamp(value: string | null | undefined): boolean {
  return value == null || validStrictRfc3339(value);
}

function validErrorCode(value: string): boolean {
  if (value === "" || utf8ByteLength(value) > 64 || rustTrim(value) !== value) {
    return false;
  }
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (!(code >= 0x61 && code <= 0x7a) && code !== 0x5f) {
      return false;
    }
  }
  return true;
}

/** 外部链接只接受无用户信息的 HTTPS URL，避免损坏投影把危险 scheme 注入浏览器。Rust 的
 * `url` 解析会把显式的空密码规范化为没有密码，因此 `password()` 为 `None` 时无需另查原文。 */
export function validForumUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" && url.hostname !== "" && url.username === "" && url.password === ""
    );
  } catch {
    return false;
  }
}

/** 严格 RFC 3339：日期与时间各字段定宽，分隔符为 T、t 或空格，偏移为 Z 或 ±hh:mm。 */
function validStrictRfc3339(text: string): boolean {
  return parseStrictRfc3339(text) !== null;
}

function rustTrim(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  let end = value.length;
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x0009 && code <= 0x000d) ||
    code === 0x0020 ||
    code === 0x0085 ||
    code === 0x00a0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
