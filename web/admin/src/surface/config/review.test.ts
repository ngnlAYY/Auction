// 配置差异按实际字段、热读兼容和未清除的重启提醒展示。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { required } from "../solid-dom-fixture";
import { configPage, settle } from "./config-page-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";
import { configChangeNeedsRestart, configChanges } from "./review";

test("review_equates_absent_and_empty_bot_players_in_both_directions", () => {
  const before = { metrics: { enabled: false } };
  const after = { metrics: { enabled: true }, bot_players: [] };
  for (const [source, candidate] of [
    [before, after],
    [after, before],
  ] as const) {
    expect(configChanges(source, candidate).map(({ path }) => path)).toEqual(["metrics.enabled"]);
  }
});

test("review_counts_removing_configured_bot_players", () => {
  const changes = configChanges({ bot_players: ["Auction Bot"] }, { bot_players: [] });
  expect(changes.map(({ path }) => path)).toEqual(["bot_players"]);
});

test("review_counts_only_edited_fields_when_materializing_early_bird_defaults", () => {
  const before = { operations: { schedule: { managed: true } } };
  // 来自 Rust early_bird::default_block，避免用被测实现生成自己的对照值。
  const earlyBird = {
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
    pre_season_session: true,
    pre_season_days_before: 3,
    qualities: [],
    pause_intake_before_season: true,
  };
  const after = { operations: { schedule: { managed: true, early_bird: earlyBird } } };
  expect(configChanges(before, after)).toEqual([]);
  earlyBird.enabled = true;
  expect(configChanges(before, after).map(({ path }) => path)).toEqual([
    "operations.schedule.early_bird.enabled",
  ]);
});

test("review_separates_actual_hot_and_restart_changes", () => {
  const before = {
    operations: {
      mail: { managed: true, send_classes: { account_reply: true, intake_reply: true } },
    },
    metrics: { enabled: false },
  };
  const after = structuredClone(before);
  after.operations.mail.send_classes.account_reply = false;
  const hotOnly = configChanges(before, after);
  expect(hotOnly).toHaveLength(1);
  expect(configChangeNeedsRestart(required(hotOnly[0], "热更新变更"))).toBe(false);

  after.metrics.enabled = true;
  const mixed = configChanges(before, after);
  expect(mixed.map(({ path }) => path)).toEqual([
    "metrics.enabled",
    "operations.mail.send_classes.account_reply",
  ]);
  expect(mixed.map(configChangeNeedsRestart)).toEqual([true, false]);
  expect(configChanges(before, before)).toEqual([]);
});

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("hot_review_includes_compatibility_and_calls_out_an_outstanding_restart", async () => {
  const fixture = await loadConfigFixture();
  const before = {
    operations: {
      forum: { managed: true, lease_duration_seconds: 600, item_timeout_seconds: 240 },
    },
  };
  const after = structuredClone(before);
  after.operations.forum.item_timeout_seconds = 360;
  for (const pendingRestart of [false, true]) {
    const page = configPage(() => {
      throw new Error("变更摘要不应发送请求");
    });
    try {
      page.mount(() =>
        fixture.createComponent(fixture.review.ConfigChangeList, {
          changes: configChanges(before, after),
          pendingRestart,
        }),
      );
      await settle();
      expect(page.root.textContent).toContain("保存后由 Worker 热读");
      expect(page.root.textContent).toContain("冲突的组沿用旧值，重启后采用新值");
      expect(page.root.textContent.includes("此前保存的配置仍待重启")).toBe(pendingRestart);
    } finally {
      page.dispose();
    }
  }
});
