// Realtime outbox 健康面板：积压、死信与最老失败年龄。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { parseStrictRfc3339 } from "../../../api/session-time";
import type { DashboardDerived } from "./derived";
import { Badge, Card, countText } from "./widgets";

const NANOS_PER_SECOND = 1_000_000_000n;

/** Realtime outbox 面板的输入；面板保持挂载，随快照与共享派生值更新。 */
export interface OutboxPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** Realtime outbox 面板；分区不可用与可用的零值必须区分展示。 */
export function OutboxPanel(props: OutboxPanelProps): JSX.Element {
  const realtime = createValueMemo(() => props.snapshot.realtime.data);
  const hasRealtimeConsumers = createValueMemo(() => props.derived.hasRealtimeConsumers);
  const pending = createValueMemo(() => props.derived.pending);
  const failures = createValueMemo(() => props.derived.failures);
  const outboxState = createValueMemo(() => props.derived.outboxState);
  const outboxStatus = createValueMemo(() => props.derived.outboxStatus);
  const pendingText = createValueMemo(() => {
    if (realtime() !== null && !hasRealtimeConsumers()) {
      return t("dashboard-legacy-outbox-status-no-consumers");
    }
    return countText(pending());
  });
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-outbox-title">
      <h2 id="dashboard-outbox-title">{t("dashboard-legacy-outbox-health")}</h2>
      <div class="admin-dashboard-cards">
        <Card
          label={t("dashboard-legacy-outbox-health")}
          value={<Badge state={outboxState()} value={outboxStatus()} />}
        />
        <Card label={t("dashboard-legacy-outbox-pending")} value={pendingText()} />
        <Card label={t("dashboard-legacy-outbox-dead-letter")} value={countText(failures())} />
      </div>
      <Show
        when={realtime()}
        fallback={
          <p class="admin-dashboard-status">{t("dashboard-legacy-outbox-metrics-unavailable")}</p>
        }
      >
        {(current) => (
          <>
            <Show when={!hasRealtimeConsumers()}>
              <p class="admin-dashboard-status">
                {t("dashboard-legacy-outbox-status-no-consumers")}
              </p>
            </Show>
            <p class="admin-dashboard-status">
              {current().failure.count === 0
                ? t("dashboard-legacy-outbox-no-unresolved-failures")
                : oldestAgeLabel(
                    oldestFailureAge(current().observed_at, current().failure.oldest_at),
                  )}
            </p>
          </>
        )}
      </Show>
      <details class="admin-dashboard-details">
        <summary>{t("dashboard-metrics-details")}</summary>
        <p class="admin-dashboard-status">{t("dashboard-uncollected-metrics")}</p>
        <h3>{t("dashboard-legacy-outbox-attempts")}</h3>
        <p class="admin-dashboard-status">{t("dashboard-legacy-metric-not-collected")}</p>
      </details>
    </section>
  );
}

function oldestAgeLabel(seconds: number | null): string {
  return seconds === null
    ? t("dashboard-legacy-no-data")
    : t("dashboard-legacy-outbox-oldest-age", { seconds });
}

function oldestFailureAge(observedAt: string, oldestAt: string | null): number | null {
  const observed = parseStrictRfc3339(observedAt);
  if (observed === null) {
    return null;
  }
  const oldest = oldestAt === null ? null : parseStrictRfc3339(oldestAt);
  if (oldest === null) {
    return null;
  }
  return observed >= oldest ? Number((observed - oldest) / NANOS_PER_SECOND) : null;
}
