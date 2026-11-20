// 运行参数区块的闭集、选填赛季前字段与扩展投影回归。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { validOperations } from "./operations";
import {
  createRuntimeConfigFixtures,
  jsonObjectAt,
  setAt,
  valueAt,
} from "./runtime-config-fixture";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(name: string): string {
  return readFileSync(new URL(`${name}.json`, FIXTURES), "utf8");
}

const { defaultOperations } = createRuntimeConfigFixtures(fixtureText);

function section(operations: Record<string, unknown>, path: string): Record<string, unknown> {
  return jsonObjectAt(operations, path);
}

test("运行参数接受完整的默认投影", () => {
  expect(validOperations(defaultOperations())).toBe(true);
});

test("运行参数拒绝缺失与多余的投影字段", () => {
  for (const [path, required, extra] of [
    ["", "worker", "extra"],
    ["/economics", "start_price", "extra"],
    ["/schedule", "open_time", "extra"],
    ["/forum", "maximum_topic_pages", "extra"],
    ["/alerts", "source_timeout_seconds", "extra"],
    ["/worker", "concurrency", "extra"],
    ["/worker/delivery", "batch_limit", "concurrency"],
    ["/worker/fulfillment", "concurrency", "extra"],
    ["/worker/recovery", "concurrency", "extra"],
    ["/mail", "send_classes", "extra"],
  ] as const) {
    const missing = defaultOperations();
    const missingSection = section(missing, path);
    expect(Object.hasOwn(missingSection, required), `${path}/${required}`).toBe(true);
    delete missingSection[required];
    expect(validOperations(missing), `${path}/${required}`).toBe(false);

    const unexpected = defaultOperations();
    const unexpectedSection = section(unexpected, path);
    expect(Object.hasOwn(unexpectedSection, extra), `${path}/${extra}`).toBe(false);
    unexpectedSection[extra] = 1;
    expect(validOperations(unexpected), `${path}/${extra}`).toBe(false);
  }
});

test("早鸟区块可独立省略赛季前场次的两个字段", () => {
  const defaults = {
    enabled: false,
    timezone: "UTC",
    worlds: ["isekai"],
    season_starts: ["05-01", "11-01"],
    season_start_time: "09:00",
    window_days: 7,
    session_days: [1, 3, 5, 7],
    open_time: "18:00",
    preview_lead_minutes: 120,
    live_minutes: 1440,
    qualities: [],
    pause_intake_before_season: true,
  };
  const withEarlyBird = (earlyBird: Record<string, unknown>): Record<string, unknown> => {
    const operations = defaultOperations();
    setAt(operations, "/schedule/early_bird", earlyBird);
    return operations;
  };
  expect(validOperations(withEarlyBird(defaults))).toBe(true);
  const explicit: Record<string, unknown> = { ...defaults, pre_season_session: false };
  expect(validOperations(withEarlyBird(explicit))).toBe(true);
  explicit.pre_season_days_before = 4;
  expect(validOperations(withEarlyBird(explicit))).toBe(true);
  delete explicit.pre_season_session;
  for (const safeInteger of [0, 4, 32]) {
    explicit.pre_season_days_before = safeInteger;
    expect(validOperations(withEarlyBird(explicit))).toBe(true);
  }

  for (const [key, invalid] of [
    ["pre_season_session", "false"],
    ["pre_season_days_before", "4"],
    ["pre_season_days_before", 9_007_199_254_740_992],
    ["pre_season_days_before", null],
    ["extra", 1],
  ] as const) {
    const earlyBird: Record<string, unknown> = { ...defaults, [key]: invalid };
    expect(validOperations(withEarlyBird(earlyBird)), key).toBe(false);
  }
  expect(Object.hasOwn(explicit, "worlds")).toBe(true);
  delete explicit.worlds;
  expect(validOperations(withEarlyBird(explicit))).toBe(false);
});

test("告警区块必须带长期未领取阈值投影", () => {
  expect(validOperations(defaultOperations())).toBe(true);
  const baseline = defaultOperations();
  const alerts = section(baseline, "/alerts");
  expect(Object.hasOwn(alerts, "fulfillment_collection_overdue_age_seconds")).toBe(true);
  delete alerts.fulfillment_collection_overdue_age_seconds;
  expect(validOperations(baseline)).toBe(false);
  for (const [seconds, valid] of [
    [0, true],
    [2_592_001, true],
    [9_007_199_254_740_992, false],
  ] as const) {
    const mutated = defaultOperations();
    setAt(mutated, "/alerts/fulfillment_collection_overdue_age_seconds", seconds);
    expect(validOperations(mutated), String(seconds)).toBe(valid);
    expect(valueAt(mutated, "/alerts/fulfillment_collection_overdue_age_seconds")).toBe(seconds);
  }
});

test("邮件区块要求服务端补齐的完整投影", () => {
  expect(validOperations(defaultOperations())).toBe(true);
  for (const key of [
    "batch_timeout_seconds",
    "send_intervals",
    "send_classes",
    "rekey_enabled",
    "account_reply_window_seconds",
    "explanation_mail_window_seconds",
    "explanation_mail_limit",
  ]) {
    const mail = defaultOperations();
    const mailBlock = section(mail, "/mail");
    expect(Object.hasOwn(mailBlock, key), `缺少 ${key} 必须拒绝`).toBe(true);
    delete mailBlock[key];
    expect(validOperations(mail), `缺少 ${key} 必须拒绝`).toBe(false);
  }
  const partialClasses = defaultOperations();
  const sendClasses = section(partialClasses, "/mail/send_classes");
  expect(Object.hasOwn(sendClasses, "player_summary")).toBe(true);
  delete sendClasses.player_summary;
  expect(validOperations(partialClasses)).toBe(false);
  const unknown = defaultOperations();
  const mail = section(unknown, "/mail");
  expect(Object.hasOwn(mail, "extra")).toBe(false);
  mail.extra = 1;
  expect(validOperations(unknown)).toBe(false);
});
