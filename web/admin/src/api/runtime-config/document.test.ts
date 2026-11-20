// 运行配置文档必需键的回归：样本直接读取 Admin 契约夹具。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { validRuntimeDocument } from "./document";
import { createRuntimeConfigFixtures } from "./runtime-config-fixture";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

const { defaultDocument } = createRuntimeConfigFixtures(fixtureText);

test("运行配置文档缺少必需键时返回失败", () => {
  const projection = defaultDocument();
  projection.mail_templates = { register_result: "旧正文" };
  expect(validRuntimeDocument(projection)).toBe(true);
  for (const key of [
    "allow_manual_early_open",
    "allow_manual_early_settlement",
    "worlds",
    "equipment_filter",
    "equipment_source",
    "listener",
    "dashboard_mailboxes",
    "metrics",
    "security",
    "session",
    "tracing",
    "worker_control",
    "operations",
  ]) {
    const missing = structuredClone(projection);
    expect(Object.hasOwn(missing, key), key).toBe(true);
    delete missing[key];
    expect(validRuntimeDocument(missing), key).toBe(false);
  }
  expect(Object.hasOwn(projection, "worlds")).toBe(true);
  expect(Object.hasOwn(projection, "equipment_filter")).toBe(true);
  delete projection.worlds;
  delete projection.equipment_filter;
  expect(validRuntimeDocument(projection)).toBe(false);
});
