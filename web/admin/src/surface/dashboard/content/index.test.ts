// 核对 Rust 看板面板缺测、零值、审计覆盖、Cookie 与投递积压的真实中文。

import { expect, test } from "bun:test";
import { createComponent } from "solid-js";
import { completePayload, dashboardSnapshot } from "../../../api/dashboard/dashboard-fixture";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { findAll } from "../../solid-dom-fixture";
import { oneT11Element, withT11View } from "../t11-solid-fixture.test";
import { deriveDashboard } from "./derived";

const OBSERVED_AT = "2026-01-01T00:01:00Z";

function realtime(oldestAt: string | null, lag: number) {
  return {
    status: "degraded",
    observed_at: OBSERVED_AT,
    data: {
      observed_at: OBSERVED_AT,
      total_lag: lag,
      lags: [
        {
          stream_key: "auction:main",
          consumer_key: "diagnostics",
          offset: 0,
          latest_sequence: lag,
          lag,
          last_advanced_at: null,
        },
      ],
      failure: {
        count: oldestAt === null ? 0 : 1,
        oldest_at: oldestAt,
        latest_code: oldestAt === null ? "" : "consumer_lag",
      },
    },
  };
}

test("missing_realtime_and_alerting_counts_are_distinct_from_measured_zeroes", async () => {
  const { SummaryMetrics: Panel } = await loadRenderModule<typeof import("./summary")>(
    new URL("./summary.tsx", import.meta.url).pathname,
    "SummaryMetrics",
  );
  const payload = completePayload();
  const missing = dashboardSnapshot({ ...payload, alerting: payload.mail });
  const measured = dashboardSnapshot({
    ...payload,
    realtime: realtime(null, 0),
    alerting: {
      ...payload.alerting,
      data: { ...payload.alerting.data, active_signal_count: 0 },
    },
  });
  for (const [snapshot, text] of [
    [missing, "暂无数据。"],
    [measured, "0"],
  ] as const) {
    await withT11View(
      () => createComponent(Panel, { snapshot, derived: deriveDashboard(snapshot) }),
      (tree) => {
        const values = findAll(
          tree,
          (element) => element.getAttribute("class") === "console-kpi__value",
        );
        expect(values).toHaveLength(6);
        expect(values.filter((element) => element.textContent === text)).toHaveLength(2);
        for (const label of ["待处理", "活跃信号"]) {
          const heading = oneT11Element(
            findAll(
              tree,
              (element) =>
                element.getAttribute("class") === "console-kpi__label" &&
                element.textContent === label,
            ),
            label,
          );
          expect(heading.parentElement?.querySelector(".console-kpi__value")?.textContent).toBe(
            text,
          );
        }
      },
    );
  }
});

test("admin_heartbeat_is_part_of_audit_coverage", async () => {
  const { AuditPanel: Panel } = await loadRenderModule<typeof import("./audit")>(
    new URL("./audit.tsx", import.meta.url).pathname,
    "AuditPanel",
  );
  const payload = completePayload();
  const incomplete = dashboardSnapshot({
    ...payload,
    health: {
      ...payload.health,
      status: "degraded",
      readiness: false,
      missing_services: ["admin"],
    },
  });
  await withT11View(
    () => createComponent(Panel, { snapshot: incomplete, derived: deriveDashboard(incomplete) }),
    (tree) => {
      expect(findAll(tree, (element) => element.textContent === "状态未知").length).toBeGreaterThan(
        0,
      );
      expect(tree.textContent).toContain("Admin API 不可用");
    },
  );
  const complete = dashboardSnapshot();
  await withT11View(
    () => createComponent(Panel, { snapshot: complete, derived: deriveDashboard(complete) }),
    (tree) => {
      for (const text of ["正常", "0"]) {
        expect(findAll(tree, (element) => element.textContent === text).length).toBeGreaterThan(0);
      }
    },
  );
});

test("cookie_database_count_does_not_imply_a_worker_restart_has_happened", async () => {
  const { CredentialsPanel: Panel } = await loadRenderModule<typeof import("./credentials")>(
    new URL("./credentials.tsx", import.meta.url).pathname,
    "CredentialsPanel",
  );
  for (const [available, databaseStatus] of [
    [0, "无活跃 Cookie"],
    [1, "已入库"],
  ] as const) {
    for (const [restartRequired, workerStatus] of [
      [true, "Worker 待重启"],
      [false, "Worker 无需重启"],
      [null, "无法确认 Worker 是否需要重启"],
    ] as const) {
      const snapshot = dashboardSnapshot({
        ...completePayload(),
        credentials: {
          status: "degraded",
          observed_at: OBSERVED_AT,
          data: {
            availability: [
              {
                world: "main",
                available,
                invalid: 0,
                restart_required: restartRequired,
                updated_at: OBSERVED_AT,
              },
            ],
            observed_at: OBSERVED_AT,
            last_admin_change_at: OBSERVED_AT,
          },
        },
      });
      await withT11View(
        () => createComponent(Panel, { snapshot }),
        (tree) => {
          for (const text of [databaseStatus, workerStatus, String(available)]) {
            expect(findAll(tree, (element) => element.textContent === text).length).toBeGreaterThan(
              0,
            );
          }
        },
      );
    }
  }
});

test("outbox_age_describes_unresolved_failure_at_observation_time", async () => {
  const { OutboxPanel: Panel } = await loadRenderModule<typeof import("./outbox")>(
    new URL("./outbox.tsx", import.meta.url).pathname,
    "OutboxPanel",
  );
  for (const [oldestAt, texts] of [
    [null, ["当前没有未解决的投递流失败。", "5"]],
    ["2026-01-01T00:00:00Z", ["最早未解决的投递流失败距快照采集时已过 60 秒"]],
  ] as const) {
    const snapshot = dashboardSnapshot({ ...completePayload(), realtime: realtime(oldestAt, 5) });
    await withT11View(
      () => createComponent(Panel, { snapshot, derived: deriveDashboard(snapshot) }),
      (tree) => {
        for (const text of texts) {
          expect(findAll(tree, (element) => element.textContent === text).length).toBeGreaterThan(
            0,
          );
        }
      },
    );
  }
});
