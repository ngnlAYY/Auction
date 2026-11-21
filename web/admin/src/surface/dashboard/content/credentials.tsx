// Cookie 状态面板：按世界展示库内凭据与 Worker 重启要求。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminCredentialAvailability, AdminDashboardSnapshot } from "../../../api/dashboard";
import { adminWorldLabel } from "../../presentation";
import { Badge } from "./widgets";

/** Cookie 状态面板的输入；面板保持挂载，随快照更新。 */
export interface CredentialsPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
}

/** Cookie 状态面板；就绪状态按响应原序，库内活跃数把 main 排在最前，其余按世界名排序。 */
export function CredentialsPanel(props: CredentialsPanelProps): JSX.Element {
  const credentials = createValueMemo(() => props.snapshot.credentials.data);
  const availability = createValueMemo(() => credentials()?.availability ?? []);
  const compareAvailability = (
    left: AdminCredentialAvailability,
    right: AdminCredentialAvailability,
  ): number => {
    if (left.world === "main" && right.world === "main") {
      return 0;
    }
    if (left.world === "main") {
      return -1;
    }
    if (right.world === "main") {
      return 1;
    }
    return compareText(left.world, right.world);
  };
  const sortedAvailability = createValueMemo(() => {
    const values = availability();
    return typeof values.toSorted === "function"
      ? values.toSorted(compareAvailability)
      : values.slice().sort(compareAvailability);
  });

  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-credentials-title">
      <h2 id="dashboard-credentials-title">{t("dashboard-legacy-cookie-readiness-by-world")}</h2>
      <Show
        when={credentials() !== null && availability().length > 0}
        fallback={<p class="admin-dashboard-status">{t("dashboard-legacy-no-data")}</p>}
      >
        <div class="admin-dashboard-cards">
          <For each={availability()} keyed={(item) => item.world}>
            {(item) => (
              <div class="admin-dashboard-card">
                <div class="admin-dashboard-value">
                  <DatabaseBadge available={item().available} invalid={item().invalid} />
                </div>
                <div class="admin-dashboard-value">
                  <WorkerRestartBadge restartRequired={item().restart_required} />
                </div>
                <div class="admin-dashboard-label">{adminWorldLabel(item().world)}</div>
              </div>
            )}
          </For>
        </div>
      </Show>

      <h3>{t("dashboard-legacy-cookies-by-world")}</h3>
      <Show
        when={sortedAvailability().length > 0}
        fallback={<p class="admin-dashboard-status">{t("dashboard-legacy-no-data")}</p>}
      >
        <div class="admin-dashboard-cards">
          <For each={sortedAvailability()} keyed={(item) => item.world}>
            {(item) => (
              <div class="admin-dashboard-card">
                <div class="admin-dashboard-value">{item().available}</div>
                <div class="admin-dashboard-label">{adminWorldLabel(item().world)}</div>
              </div>
            )}
          </For>
        </div>
      </Show>
    </section>
  );
}

function DatabaseBadge(props: {
  readonly available: number;
  readonly invalid: number;
}): JSX.Element {
  const status = createValueMemo(() =>
    props.available > 0
      ? { state: "ok", value: t("dashboard-legacy-cookie-stored") }
      : props.invalid > 0
        ? { state: "error", value: t("dashboard-legacy-cookie-undecryptable") }
        : { state: "warn", value: t("dashboard-legacy-cookie-no-active") },
  );
  return <Badge state={status().state} value={status().value} />;
}

function WorkerRestartBadge(props: {
  readonly restartRequired: AdminCredentialAvailability["restart_required"];
}): JSX.Element {
  // 停用最后一条凭据也可能需要重启，库内活跃数不能代替 Worker 状态。
  const status = createValueMemo(() => {
    if (props.restartRequired === true) {
      return { state: "warn", value: t("dashboard-legacy-cookie-restart-required") };
    }
    if (props.restartRequired === false) {
      return { state: "ok", value: t("dashboard-legacy-cookie-restart-not-required") };
    }
    return { state: "neutral", value: t("dashboard-legacy-cookie-unconfirmed") };
  });
  return <Badge state={status().state} value={status().value} />;
}
