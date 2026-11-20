// 结算、恢复与退件测试共用的契约样本和只读请求夹具。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import type { GenericSchema, InferOutput } from "valibot";

import { createAdminRequestHarness, parseJsonObject, removeAt } from "./api-request-fixture";
import type { AdminSessionHost } from "./session";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

/** 读取 Rust 测试共用的原始响应样本。 */
export function adminContract(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

/** 从契约样本替换纳秒整数的原文，避免构造测试输入时提前舍入。 */
export function settlementLongHoldBody(lexeme: string): string {
  const body = adminContract("settlement-report-long-hold");
  const field = '"leader_held_nanos": 9072000000000000';
  expect(body.split(field)).toHaveLength(2);
  return body.replace(field, `"leader_held_nanos": ${lexeme}`);
}

/** 在同一 schema 上检查完整样本和缺少必填键后的拒绝，再返回成功值。 */
export function checkedContract<T extends GenericSchema>(
  body: string,
  schema: T,
  parent: string,
  key: string,
): InferOutput<T> {
  const envelope = apiResponseSchema(schema);
  const decodeResponse = (text: string) => decode(envelope, requireOk(parseJson(text)));
  const response = requireOk(decodeResponse(body));
  const invalid = parseJsonObject(body, "契约样本");
  removeAt(invalid, `${parent}/${key}`);
  expect(decodeResponse(JSON.stringify(invalid)).ok, `缺少 ${parent}/${key}`).toBe(false);
  return response.data;
}

/** 先断言结果成功，再取得值；夹具坏掉时立即给出失败。 */
export function requireOk<T>(
  result: { readonly ok: true; readonly value: T } | { readonly ok: false },
): T {
  expect(result.ok).toBe(true);
  if (!result.ok) {
    throw new Error("测试需要成功结果");
  }
  return result.value;
}

/** 走真实读取入口并核对唯一的 GET，之后恢复宿主和传输。 */
export async function readAdminFixture<T>(
  body: string,
  path: string,
  read: (host: AdminSessionHost, signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const fixture = createAdminRequestHarness();
  try {
    const { host } = fixture.registerHost();
    const requests = fixture.installFetch([{ body }]);
    const result = await read(host, new AbortController().signal);
    expect(requests).toEqual([{ url: path, method: "GET", body: "" }]);
    return result;
  } finally {
    fixture.cleanup();
  }
}

test("纳秒夹具按原词法替换，生成超出安全整数范围的输入时不舍入", () => {
  const before = adminContract("settlement-report-long-hold");
  const after = settlementLongHoldBody("9223372036854775807");
  const [prefix, suffix] = before.split('"leader_held_nanos": 9072000000000000');
  expect(after).toBe(`${prefix}"leader_held_nanos": 9223372036854775807${suffix}`);
});
