// 登录页注册账号的匿名读取；只接收公开名单，不读取浏览器外的配置文件。

import {
  type PublicRegistrationInfoResponse,
  PublicRegistrationInfoResponseSchema,
} from "@auction/contracts/dto/public/account";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { jsonExactObject } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { sendGet } from "./clock";
import { decodeErrorCode, decodeSuccess } from "./decoder";

/** 读取注册页的匿名配置；沿用同源请求总期限，失败不能用示例账号替代配置。 */
export async function fetchRegistrationInfo(
  signal: AbortSignal,
): Promise<Result<PublicRegistrationInfoResponse, string>> {
  const sent = await sendGet("/api/v1/registration-info", signal);
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
  return decodeSuccess(response, PublicRegistrationInfoResponseSchema, validRegistrationInfo);
}

function validRegistrationInfo(value: unknown): boolean {
  const object = jsonExactObject(value, ["bot_players"]);
  if (object === null) {
    return false;
  }
  const players = object.bot_players;
  if (!Array.isArray(players) || players.length > 16) {
    return false;
  }
  return players.every((candidate, index) => {
    if (typeof candidate !== "string") {
      return false;
    }
    const name = candidate;
    if (
      name.length === 0 ||
      utf8ByteLength(name) > 64 ||
      rustTrim(name) !== name ||
      hasControlCharacter(name)
    ) {
      return false;
    }
    for (let previousIndex = 0; previousIndex < index; previousIndex += 1) {
      const previous = players[previousIndex];
      if (typeof previous === "string" && equalsAsciiIgnoreCase(previous, name)) {
        return false;
      }
    }
    return true;
  });
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

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) {
      return true;
    }
  }
  return false;
}

function equalsAsciiIgnoreCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    let leftCode = left.charCodeAt(index);
    let rightCode = right.charCodeAt(index);
    if (leftCode >= 65 && leftCode <= 90) {
      leftCode += 32;
    }
    if (rightCode >= 65 && rightCode <= 90) {
      rightCode += 32;
    }
    if (leftCode !== rightCode) {
      return false;
    }
  }
  return true;
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
