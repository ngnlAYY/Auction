// 邮件诊断扩展的闭集校验；不接受原始游标或跨邮箱的迟到观测。

import type {
  AdminMailPollingDiagnosticsResponse,
  AdminMailPollProgressResponse,
} from "@auction/contracts/dto/admin/mail/progress";
import { utf8ByteLength } from "@auction/web-shared/text";

import { parseAdminTime } from "./validation";

/** 数字 MID 的安全边界：非空、最长 64 字节、首位为 1–9、其余全为 ASCII 数字。 */
function safeMid(value: string): boolean {
  if (value.length === 0 || utf8ByteLength(value) > 64) {
    return false;
  }
  const first = value.charCodeAt(0);
  if (first < 0x31 || first > 0x39) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 只有读取详情和处理阶段允许携带 MID。 */
function mailStage(stage: string): boolean {
  return stage === "reading_detail" || stage === "processing";
}

/** 失败定位只允许仍在推进的入站阶段。 */
function activeStage(stage: string): boolean {
  return (
    stage === "fetching" ||
    stage === "reading_detail" ||
    stage === "processing" ||
    stage === "committing"
  );
}

/** MID 只能出现在读取详情或处理阶段；其他阶段的目标必须是空。 */
function safeTarget(stage: string, mid: string | null): boolean {
  return (mid === null || safeMid(mid)) && (mailStage(stage) || mid === null);
}

/** 校验诊断扩展与邮箱、游标和 Worker 观测时间的对应关系。 */
export function validDiagnostics(
  value: AdminMailPollingDiagnosticsResponse,
  mailbox: string,
): boolean {
  const observedAt = parseAdminTime(value.observed_at);
  if (observedAt === null) {
    return false;
  }
  let cursorValid: boolean;
  switch (value.cursor_status) {
    case "available":
      cursorValid =
        value.completed_external_mail_id !== null && safeMid(value.completed_external_mail_id);
      break;
    case "empty":
    case "hidden":
      cursorValid = value.completed_external_mail_id === null;
      break;
    default:
      cursorValid = false;
  }
  if (!cursorValid) {
    return false;
  }
  if (value.worker_condition !== "available") {
    return (
      (value.worker_condition === "unavailable" ||
        value.worker_condition === "timeout" ||
        value.worker_condition === "incompatible" ||
        value.worker_condition === "rejected") &&
      value.worker_observed_at === null &&
      value.progress === null
    );
  }
  const workerAt =
    value.worker_observed_at === null ? null : parseAdminTime(value.worker_observed_at);
  if (workerAt === null || workerAt > observedAt) {
    return false;
  }
  return value.progress === null || validProgress(value.progress, mailbox, workerAt);
}

function validProgress(
  value: AdminMailPollProgressResponse,
  mailbox: string,
  observedAt: number,
): boolean {
  return (
    value.mailbox_key === mailbox &&
    (value.world_id === "main" || value.world_id === "isekai") &&
    (activeStage(value.stage) ||
      value.stage === "idle" ||
      value.stage === "failed" ||
      value.stage === "canceled") &&
    safeTarget(value.stage, value.external_mail_id) &&
    parseIsAtOrBefore(value.updated_at, observedAt) &&
    (value.last_failure === null ||
      (activeStage(value.last_failure.stage) &&
        safeTarget(value.last_failure.stage, value.last_failure.external_mail_id) &&
        parseIsAtOrBefore(value.last_failure.occurred_at, observedAt)))
  );
}

function parseIsAtOrBefore(value: string, observedAt: number): boolean {
  const parsed = parseAdminTime(value);
  return parsed !== null && parsed <= observedAt;
}
