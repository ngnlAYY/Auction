// T11 直接读取 Rust 与 Solid 共用的 Admin 契约样本。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJsonObject } from "./api-request-fixture";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

/** 读入原始样本文本，保留线上数字词法。 */
export function adminFixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

/** 用共享解析器读取样本，供测试改写或调用数据解码入口。 */
export function adminFixtureBody(name: string): Record<string, unknown> {
  return parseJsonObject(adminFixtureText(name), name);
}

test("契约夹具每次读出独立对象，可删除字段构造负例", () => {
  const edited = adminFixtureBody("dashboard");
  delete edited.data;
  expect(adminFixtureBody("dashboard")).toMatchObject({ code: "ok", data: expect.any(Object) });
  expect(edited).not.toHaveProperty("data");
});
