// 运行配置的共享契约样本、线上信封和文档字段操作。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  type AdminJsonValue,
  type AdminRuntimeConfigResponse,
  AdminRuntimeConfigResponseSchema,
} from "@auction/contracts/dto/admin/rules";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { parseAdminRuntimeConfigDocumentText } from "../../api/runtime-config";

const FIXTURE_ROOT = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

/** Rust 和 Solid 读取配置用的查询投影。 */
export const CONFIG_PROJECTION_QUERY =
  "?include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";

/** Rust 测试读取助手用 unknown 补齐 GET 的扩展投影。 */
export function configReadReply(snapshot: ReturnType<typeof defaultProjection>): Response {
  return Response.json({
    ...snapshot,
    data: { worker_restart_state: "unknown", ...snapshot.data },
  });
}

/** 在运行期核对结果后取出值，错误直接使测试失败。 */
export function accepted<T>(
  result:
    | { readonly ok: true; readonly value: T }
    | { readonly ok: false; readonly error?: unknown },
): T {
  if (!result.ok) {
    throw new Error(`夹具结果失败 ${JSON.stringify(result.error)}`);
  }
  return result.value;
}

/** 读取 Rust 同一份完整线上样本，保留省略键和原设备分类。 */
export function defaultProjection() {
  const raw = jsonObject(
    accepted(parseJson(readFileSync(new URL("runtime-config-default.json", FIXTURE_ROOT), "utf8"))),
  );
  return { ...raw, data: jsonObject(raw.data) };
}

/** DTO 解码发生在文档模型装配之前，和 Rust 的测试夹具入口一致。 */
export function decodeConfigData(data: unknown): AdminRuntimeConfigResponse {
  return accepted(decode(AdminRuntimeConfigResponseSchema, data));
}

/** 对照 Rust 的读取助手，从契约信封中取得经过检查的完整投影。 */
export function decodeConfigProjection(projection: unknown): AdminRuntimeConfigResponse {
  return accepted(decode(apiResponseSchema(AdminRuntimeConfigResponseSchema), projection)).data;
}

/** 动态字段读写只在检查过的 JSON 对象上执行。 */
export function jsonObject(value: unknown): Record<string, unknown> {
  if (!isObject(value)) {
    throw new Error("预期 JSON 对象");
  }
  return value;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** 找到点分字段所在的 JSON 对象。 */
export function objectAt(root: unknown, path: string): Record<string, unknown> {
  let current = jsonObject(root);
  for (const segment of path.split(".")) {
    current = jsonObject(current[segment]);
  }
  return current;
}

/** 读取文档字段，路径内的对象或数组必须实际存在。 */
export function valueAt(root: unknown, path: string): unknown {
  let current = root;
  for (const segment of path.split(".")) {
    if (Array.isArray(current)) {
      current = current[Number(segment)];
    } else {
      current = jsonObject(current)[segment];
    }
  }
  return current;
}

/** 运行期检查自由 JSON 文档，保留 Rust 样本的原始字段。 */
export function documentValue(document: unknown): AdminJsonValue {
  return accepted(parseAdminRuntimeConfigDocumentText(JSON.stringify(document)));
}

/** 测试 HTTP 失败信封；以 Rust 夹具使用的线上字段构造。 */
export function apiFailure(status: number, code: string, message: string): Response {
  return Response.json({ code, message, data: null, request_id: null }, { status });
}

test("配置读取夹具只在样本省略重启状态时补齐默认值", async () => {
  const snapshot = defaultProjection();
  const readState = async (response: Response) =>
    valueAt(accepted(parseJson(await response.text())), "data.worker_restart_state");
  expect(snapshot.data).not.toHaveProperty("worker_restart_state");
  expect(await readState(configReadReply(snapshot))).toBe("unknown");
  const explicit = { ...snapshot, data: { ...snapshot.data, worker_restart_state: "consistent" } };
  expect(await readState(configReadReply(explicit))).toBe("consistent");
});
