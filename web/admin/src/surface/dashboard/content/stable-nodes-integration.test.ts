// 用编译后的真实面板核对快照刷新时指标卡、徽标与业务键行的节点身份。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createComponent } from "solid-js";
import type { AdminProcessData, AdminRecoveryWorld } from "../../../api/dashboard";
import { completePayload, dashboardSnapshot } from "../../../api/dashboard/dashboard-fixture";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { findAll, type TestElement } from "../../solid-dom-fixture";
import { oneT11Element, settleT11View, withT11View } from "../t11-solid-fixture.test";
import { deriveDashboard } from "./derived";

const OBSERVED_AT = "2026-01-01T00:00:00Z";

function heartbeat(instanceId: string): AdminProcessData {
  return {
    service_name: "admin",
    process_role: "admin",
    instance_id: instanceId,
    version: "0.1.0",
    started_at: OBSERVED_AT,
    status: "ready",
    readiness: true,
    audit_failure_count: 0,
    tracing: {
      status: "ok",
      dropped_span_count: 0,
      export_failure_count: 0,
      last_success_at: null,
      last_failure_at: null,
      last_error_code: "",
    },
    observed_at: OBSERVED_AT,
    expires_at: "2026-01-01T00:01:00Z",
  };
}

function processSnapshot(...processes: AdminProcessData[]) {
  const payload = completePayload();
  return dashboardSnapshot({
    ...payload,
    process: {
      status: "ok",
      observed_at: OBSERVED_AT,
      data: { processes, summary: payload.health, observed_at: OBSERVED_AT },
    },
  });
}

function recoveryWorld(world: string): AdminRecoveryWorld {
  return {
    world,
    balance_credits: 50,
    balance_status: "ok",
    evidence_status: "fresh",
    balance_observed_at: OBSERVED_AT,
    evidence_last_success_at: OBSERVED_AT,
    updated_at: OBSERVED_AT,
  };
}

function recoverySnapshot(overdue: number, ...worlds: AdminRecoveryWorld[]) {
  return dashboardSnapshot({
    ...completePayload(),
    recovery: {
      status: "ok",
      observed_at: OBSERVED_AT,
      data: {
        observed_at: OBSERVED_AT,
        worlds,
        payment_overdue_count: overdue,
        relist_pending_count: 2,
        failed_action_count: 3,
        manual_reconcile_count: 4,
        oldest_overdue_age_seconds: 60,
      },
    },
  });
}

function badges(tree: TestElement): TestElement[] {
  return findAll(tree, (element) => {
    // 现有 universal 夹具保留 class 的数组参数，浏览器会把它合并成 class 字符串。
    const classes = element.attributes.class;
    return (
      element.tag === "span" &&
      (Array.isArray(classes)
        ? classes.includes("admin-dashboard-badge")
        : element.matches(".admin-dashboard-badge"))
    );
  });
}

function expectBadge(node: TestElement, state: string, value: string): void {
  const classes = node.attributes.class;
  expect(Array.isArray(classes) ? classes.join(" ") : classes).toBe(
    `admin-dashboard-badge admin-dashboard-badge--${state}`,
  );
  expect(node.textContent).toBe(value);
}

function worldBalanceBadge(row: TestElement): TestElement {
  return oneT11Element(
    badges(row).filter((node) => node.parentElement?.matches(".admin-dashboard-recovery-world")),
    "余额徽标",
  );
}

function worldEvidenceBadge(row: TestElement): TestElement {
  return oneT11Element(
    badges(row).filter((node) => node.parentElement?.matches(".admin-dashboard-label")),
    "证据徽标",
  );
}

test("同一追踪实例刷新计数和状态时复用徽标", async () => {
  const { TracingPanel: Panel } = await loadRenderModule<typeof import("./tracing")>(
    new URL("tracing.tsx", import.meta.url).pathname,
    "TracingPanel",
  );
  const instance = heartbeat("admin-1");
  const snapshot = createCell(processSnapshot(instance));
  await withT11View(
    () =>
      createComponent(Panel, {
        get snapshot() {
          return snapshot();
        },
      }),
    async (tree) => {
      const row = oneT11Element(tree.querySelectorAll(".admin-dashboard-card"), "进程行");
      const badge = oneT11Element(badges(row), "追踪徽标");
      const stack = oneT11Element(row.querySelectorAll(".admin-dashboard-metric-stack"), "指标");
      const counts = oneT11Element(stack.querySelectorAll(".admin-dashboard-label"), "计数");
      expectBadge(badge, "ok", "正常");
      expect(counts.textContent).toBe("丢弃 0 · 导出失败 0");

      const tracing = { ...instance.tracing, dropped_span_count: 4, export_failure_count: 2 };
      snapshot.set(processSnapshot({ ...instance, tracing }));
      await settleT11View();
      expect(tree.querySelector(".admin-dashboard-card")).toBe(row);
      expect(oneT11Element(badges(row), "计数变化后的徽标")).toBe(badge);
      expectBadge(badge, "ok", "正常");
      expect(counts.textContent).toBe("丢弃 4 · 导出失败 2");

      snapshot.set(processSnapshot({ ...instance, tracing: { ...tracing, status: "degraded" } }));
      await settleT11View();
      expect(tree.querySelector(".admin-dashboard-card")).toBe(row);
      expect(oneT11Element(badges(row), "状态变化后的徽标")).toBe(badge);
      expectBadge(badge, "error", "需要处理");
    },
  );
});

test("同一服务的追踪实例增减时按实例键复用其余行", async () => {
  const { TracingPanel: Panel } = await loadRenderModule<typeof import("./tracing")>(
    new URL("tracing.tsx", import.meta.url).pathname,
    "TracingPanel",
  );
  const first = heartbeat("admin-1");
  const second = heartbeat("admin-2");
  const snapshot = createCell(processSnapshot(first));
  await withT11View(
    () =>
      createComponent(Panel, {
        get snapshot() {
          return snapshot();
        },
      }),
    async (tree) => {
      const rows = () => tree.querySelectorAll(".admin-dashboard-card");
      const firstRow = oneT11Element(rows(), "首个实例");
      snapshot.set(processSnapshot(second, first));
      await settleT11View();
      expect(rows()).toHaveLength(2);
      const secondRow = oneT11Element(rows().slice(0, 1), "新实例");
      expect(secondRow).not.toBe(firstRow);
      expect(rows()[1]).toBe(firstRow);

      snapshot.set(processSnapshot(second));
      await settleT11View();
      expect(oneT11Element(rows(), "剩余实例")).toBe(secondRow);
    },
  );
});

test("恢复指标卡与世界徽标原位更新，世界增减只影响对应行", async () => {
  const { RecoveryPanel: Panel } = await loadRenderModule<typeof import("./recovery")>(
    new URL("recovery.tsx", import.meta.url).pathname,
    "RecoveryPanel",
  );
  const main = recoveryWorld("main");
  const isekai = recoveryWorld("isekai");
  const snapshot = createCell(recoverySnapshot(1, main));
  await withT11View(
    () =>
      createComponent(Panel, {
        get snapshot() {
          return snapshot();
        },
        get derived() {
          return deriveDashboard(snapshot());
        },
      }),
    async (tree) => {
      const metrics = () => tree.querySelectorAll(".admin-dashboard-cards")[0];
      const worldRows = () =>
        tree
          .querySelectorAll(".admin-dashboard-cards")[1]
          ?.querySelectorAll(".admin-dashboard-card") ?? [];
      const metricsGroup = metrics();
      const cards = metricsGroup?.querySelectorAll(".admin-dashboard-card") ?? [];
      expect(cards).toHaveLength(6);
      const health = oneT11Element(cards.slice(0, 1), "恢复健康卡");
      const healthBadge = oneT11Element(badges(health), "恢复健康徽标");
      const overdue = oneT11Element(cards.slice(1, 2), "付款逾期卡");
      const overdueValue = overdue.querySelector(".admin-dashboard-value");
      expect(overdueValue?.textContent).toBe("1");
      expect(overdue.querySelector(".admin-dashboard-label")?.textContent).toBe("付款逾期");
      expectBadge(healthBadge, "ok", "正常");
      const mainRow = oneT11Element(worldRows(), "主世界行");
      const balance = worldBalanceBadge(mainRow);
      const evidence = worldEvidenceBadge(mainRow);
      expectBadge(balance, "ok", "余额正常");
      expectBadge(evidence, "ok", "新鲜");

      snapshot.set(recoverySnapshot(8, { ...main, balance_credits: 75 }));
      await settleT11View();
      expect(metrics()).toBe(metricsGroup);
      for (const [index, card] of cards.entries()) {
        expect(metrics()?.querySelectorAll(".admin-dashboard-card")[index]).toBe(card);
      }
      expect(overdue.querySelector(".admin-dashboard-value")).toBe(overdueValue);
      expect(overdueValue?.textContent).toBe("8");
      expect(oneT11Element(worldRows(), "余额变化后的主世界行")).toBe(mainRow);
      expect(mainRow.textContent).toContain("余额：75 credits");
      expect(worldBalanceBadge(mainRow)).toBe(balance);
      expect(worldEvidenceBadge(mainRow)).toBe(evidence);
      expectBadge(balance, "ok", "余额正常");
      expectBadge(evidence, "ok", "新鲜");

      snapshot.set(
        recoverySnapshot(8, isekai, { ...main, balance_status: "busy", evidence_status: "stale" }),
      );
      await settleT11View();
      expect(worldRows()).toHaveLength(2);
      const isekaiRow = oneT11Element(worldRows().slice(0, 1), "异世界行");
      expect(isekaiRow).not.toBe(mainRow);
      expect(worldRows()[1]).toBe(mainRow);
      expect(worldBalanceBadge(mainRow)).toBe(balance);
      expect(worldEvidenceBadge(mainRow)).toBe(evidence);
      expectBadge(balance, "warn", "账号战斗中");
      expectBadge(evidence, "warn", "过期");
      expect(mainRow.textContent).toContain("请先结束该世界的战斗");
      expect(metrics()?.querySelector(".admin-dashboard-card")).toBe(health);
      expect(oneT11Element(badges(health), "状态变化后的恢复健康徽标")).toBe(healthBadge);
      expectBadge(healthBadge, "warn", "需要处理");

      snapshot.set(recoverySnapshot(8, isekai));
      await settleT11View();
      expect(oneT11Element(worldRows(), "剩余世界行")).toBe(isekaiRow);
    },
  );
});
