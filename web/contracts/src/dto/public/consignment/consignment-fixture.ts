// 测试读取 Rust 和 TypeScript 共用的脱敏寄售 JSON 样本。

import type { GenericSchema, InferOutput } from "valibot";

import { decode } from "../../../wire";
import { apiResponseSchema } from "../../response";

const FIXTURES = new URL(
  "../../../../../../crates/auction-contracts/tests/fixtures/",
  import.meta.url,
);

/** 解码夹具，失败时只报告文件名，避免在测试输出中回显请求。 */
export async function readFixture<TSchema extends GenericSchema>(
  file: string,
  schema: TSchema,
): Promise<InferOutput<TSchema>> {
  const input: unknown = await Bun.file(new URL(file, FIXTURES)).json();
  const decoded = decode(schema, input);
  if (!decoded.ok) throw new Error(`夹具解码失败：${file}`);
  return decoded.value;
}

/** 响应夹具经统一成功外壳与具体数据 schema 解码。 */
export function readResponse<TSchema extends GenericSchema>(file: string, schema: TSchema) {
  return readFixture(file, apiResponseSchema(schema));
}
