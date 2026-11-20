// Admin realtime 失败重放的正文冻结与发送。
// 公开 JSON 在提交时解析、校验并深冻结；同一 command_id 的重试只复用第一次生成的正文。

import type { AdminRealtimeReplayRequest } from "@auction/contracts/dto/admin/realtime";
import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { parseRustUuid, rustUuidIsNil } from "../credentials/decode";
import { type AdminApiFailure, adminApiFailureClient, invalidRequestError } from "../failure";
import type { AdminSessionHost } from "../session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendPreparedJsonWrite,
} from "../transport";
import { decodeRealtimeReplayReceipt } from "./decode";

const I32_MIN = -2_147_483_648;
const I32_MAX = 2_147_483_647;

/** 已冻结的重放参数与首次执行时生成的正文；正文供首次发送和原样重试共用。 */
export interface PreparedRealtimeReplay {
  readonly command: PreparedJsonCommand;
  readonly schemaVersion: number;
}

/**
 * 重放一次失败事件。
 *
 * 正文在提交时冻结：解析后的公开 JSON、schema 版本与命令 ID 都不再读取草稿；
 * 第一次执行时由 `prepareJsonCommandOnce` 生成正文，后续重试复用同一份文本。
 */
export function prepareRealtimeReplay(
  schemaVersion: number,
  publicPayloadText: string,
): (commandId: string) => Result<PreparedRealtimeReplay, AdminApiFailure> {
  if (!Number.isInteger(schemaVersion) || schemaVersion < I32_MIN || schemaVersion > I32_MAX) {
    const failure = adminApiFailureClient("invalid_request");
    return () => err(failure);
  }
  const parsed = parseJson(publicPayloadText, { uniqueKeys: "all" });
  if (!parsed.ok || !isJsonObject(parsed.value) || !replayNumbersLossless(publicPayloadText)) {
    const failure = adminApiFailureClient(invalidRequestError(parsed));
    return () => err(failure);
  }
  let payload: AdminJsonValue;
  try {
    // parseJson 已证明值是合法 JSON，且上方已确认根是普通对象；与 serde_json::Value 同形。
    payload = deepFreeze(structuredClone(parsed.value)) as AdminJsonValue;
  } catch (error) {
    const failure = adminApiFailureClient(invalidRequestError(error));
    return () => err(failure);
  }
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminRealtimeReplayRequest = {
      command_id: commandId,
      schema_version: schemaVersion,
      public_payload: payload,
    };
    return command;
  });
  return (commandId) => {
    // Rust 的请求字段是 `Uuid`：解析后写回规范形式，保证 v7 等版本与大小写拼写都能发送。
    const canonicalCommandId = parseRustUuid(commandId);
    if (canonicalCommandId === null || rustUuidIsNil(canonicalCommandId)) {
      return err(adminApiFailureClient("invalid_request"));
    }
    const prepared = prepare(canonicalCommandId);
    return prepared.ok ? ok({ command: prepared.value, schemaVersion }) : prepared;
  };
}

/** 发送一次重放命令。结果未知的传输错误原样交给未决命令宿主。 */
export async function replayRealtimeFailure(
  host: AdminSessionHost,
  failureId: string,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedRealtimeReplay, AdminApiFailure>,
  signal: AbortSignal,
): Promise<Result<void, AdminApiFailure>> {
  const canonicalCommandId = parseRustUuid(commandId);
  if (canonicalCommandId === null || rustUuidIsNil(canonicalCommandId)) {
    return err(adminApiFailureClient("invalid_request"));
  }
  const prepared = prepare(canonicalCommandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    `/api/v1/admin/realtime/failures/${failureId}/replay`,
    "POST",
    prepared.value.command,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const decoded = decodeRealtimeReplayReceipt(response.value, prepared.value.schemaVersion);
  return decoded.ok ? ok(undefined) : err(adminApiFailureClient(decoded.error));
}

function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function deepFreeze(value: unknown): unknown {
  if (Array.isArray(value)) {
    for (const item of value) {
      deepFreeze(item);
    }
    Object.freeze(value);
  } else if (isJsonObject(value)) {
    for (const key of Object.keys(value)) {
      deepFreeze(value[key]);
    }
    Object.freeze(value);
  }
  return value;
}

/**
 * 自由 JSON 的数字必须原样提交：解析后再序列化与原文不同即拒绝。
 * 覆盖超出安全整数的整数、被舍入的浮点尾数、`-0` 以及 `1.0`、`1e3` 这类会被改写的写法。
 */
function replayNumbersLossless(text: string): boolean {
  let index = 0;
  while (index < text.length) {
    const character = text.charAt(index);
    if (character === '"') {
      index = skipJsonString(text, index);
      continue;
    }
    if (character === "-" || (character >= "0" && character <= "9")) {
      NUMBER_AT.lastIndex = index;
      const match = NUMBER_AT.exec(text);
      if (match === null) {
        return false;
      }
      const lexeme = match[0];
      // 词法已由正则限定为 JSON 数字；这里只做数值转换，不重新解析整份 JSON。
      const value = Number(lexeme);
      if (!Number.isFinite(value)) {
        return false;
      }
      if (!/[.eE]/u.test(lexeme) && !Number.isSafeInteger(value)) {
        return false;
      }
      if (JSON.stringify(value) !== lexeme) {
        return false;
      }
      index += lexeme.length;
      continue;
    }
    index += 1;
  }
  return true;
}

const NUMBER_AT = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/y;

/** 跳过 JSON 字符串（含转义）；输入已由 `parseJson` 证明是合法 JSON。 */
function skipJsonString(text: string, start: number): number {
  let index = start + 1;
  while (index < text.length) {
    if (text[index] === "\\") {
      index += 2;
    } else if (text[index] === '"') {
      return index + 1;
    } else {
      index += 1;
    }
  }
  return index;
}
