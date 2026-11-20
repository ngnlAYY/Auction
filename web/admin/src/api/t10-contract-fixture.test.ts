// T10 接口测试共用的契约样本与缓冲响应。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import type { Decoded } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";
import { parseJsonObject, removeAt } from "./api-request-fixture";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

/** 读取 Rust Admin 契约测试使用的同一份样本。 */
export function adminFixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

/** 确认目标键存在，再生成仅省略该键的负例。 */
export function withoutFixtureField(text: string, path: string): string {
  const data = parseJsonObject(text, "契约样本");
  removeAt(data, path);
  return JSON.stringify(data);
}

/** 构造生产解码器使用的已缓冲响应，保留原始 JSON 及调用方的字段策略。 */
export function bufferedAdminResponse(text: string): TransportResponse {
  return {
    status: 200,
    statusText: "",
    headers: new Headers(),
    body: new TextEncoder().encode(text),
    text: () => text,
    json(policy) {
      const parsed = parseJson(text, policy);
      return parsed.ok ? parsed : err({ kind: "decode" });
    },
  };
}

/** 先确认操作成功，再解包结果供测试断言。 */
export function mustSucceed<T>(result: Result<T, unknown> | Decoded<T>): T {
  if (!result.ok) {
    throw new Error(
      "error" in result ? `期望成功，实际错误：${String(result.error)}` : "期望 DTO 解码成功",
    );
  }
  return result.value;
}

test("缓冲回执保留原始字节并应用调用方传入的 JSON 字段策略", () => {
  const text = '{"version":1,"version":2}';
  const response = bufferedAdminResponse(text);
  expect(response.text()).toBe(text);
  expect(response.body).toEqual(new TextEncoder().encode(text));
  expect(response.json({ uniqueKeys: "all" })).toEqual({ ok: false, error: { kind: "decode" } });
});
