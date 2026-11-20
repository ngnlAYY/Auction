// Admin 告警台页面。
// 活动/历史列表、确认与带到期的静默、处置手册锚点。静默只抑制重复通知，不修改阈值或
// signal 状态；服务端仍是唯一判定者，页面只展示安全投影。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, onCleanup, Show, Switch } from "solid-js";

import { type AdminAlertPage, fetchAdminAlerts } from "../../api/alerts";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import {
  type AdminActionFeedback,
  adminActionFeedbackIsSuccess,
  adminActionFeedbackMessage,
  adminApiErrorMessage,
} from "../presentation";
import { type AlertsQuery, alertStatusChoice, useAlertsQueryState } from "../query/alerts";
import { useAlertReasonDraft } from "./reason";
import { AlertTable } from "./table";
import { WebhookTable } from "./webhooks";

type AlertsView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminAlertPage };

export function page(): JSX.Element {
  const host = useAdminSessionHost();
  // 状态筛选与分页跟地址栏双向同步；备注、静默时长与命令反馈是瞬态，不入 URL。
  const query = useAlertsQueryState();
  const queryState = query.state;
  const refresh = createCommandReload(0n);
  const notice = createCell<AdminActionFeedback | null>(null);
  const reason = useAlertReasonDraft();
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  // 命令成功后宿主递增 refresh，列表与 webhook 视图随之重读。
  const pending = usePendingCommands(refresh, (entity) => entity.startsWith("alert:"));
  const pagination = useAdminPagination("alerts", ["page_size"], 100, 100_000);
  const alerts = useAdminPage(
    pagination,
    () => {
      refresh();
      return queryState();
    },
    async (request: AlertsQuery, size, signal) => {
      try {
        return await fetchAdminAlerts(host, request.status, Number(request.page), size, signal);
      } catch {
        // 读取异常按稳定传输失败收尾，现有错误视图与重试入口继续生效。
        return err("network_error");
      }
    },
  );
  const view = createValueMemo((): AlertsView => {
    const current = alerts();
    if (current === null) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "error", code: current.error };
    }
    return { kind: "ready", page: current.value };
  });
  const errorView = createValueMemo((): { readonly code: string } | null => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  const readyView = createValueMemo((): { readonly page: AdminAlertPage } | null => {
    const current = view();
    return current.kind === "ready" ? current : null;
  });
  const statusQuery = query;

  return (
    <PendingCommandsContext value={pending}>
      <section class="admin-alert-console-page" aria-labelledby="alert-console-title">
        <h1 id="alert-console-title">{t("page-alerts-title")}</h1>
        <p class="admin-alert-console__hint">{t("alert-console-hint")}</p>
        <PendingCommandPanel owner={pending} />
        <div class="admin-alert-console__filters">
          <label>
            {t("alert-console-status")}
            <select
              value={queryState().status}
              onChange={(event) => {
                // 未知值不改变筛选；切换筛选回到第一页，避免落在不存在的深页。
                const status = alertStatusChoice(event.currentTarget.value);
                if (status !== null) {
                  statusQuery.push({ status, page: 1n });
                }
              }}
            >
              <option value="active">{t("alert-console-status-active")}</option>
              <option value="resolved">{t("alert-console-status-resolved")}</option>
              <option value="all">{t("alert-console-status-all")}</option>
            </select>
          </label>
        </div>
        <Show when={notice()}>
          {(feedback) => (
            <p
              class={
                adminActionFeedbackIsSuccess(feedback())
                  ? "action-feedback action-feedback--success"
                  : "action-feedback action-feedback--error"
              }
              role={adminActionFeedbackIsSuccess(feedback()) ? "status" : "alert"}
            >
              {adminActionFeedbackMessage(feedback(), t("alert-console-action-ok"))}
            </p>
          )}
        </Show>
        <Switch
          fallback={<AsyncState kind="loading" title={t("alert-console-loading")} detail="" />}
        >
          <Match when={errorView()}>
            {(failure) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(failure().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(refresh)}
              />
            )}
          </Match>
          <Match when={readyView()}>
            {(current) => (
              <>
                <AlertTable
                  page={current().page}
                  reason={reason}
                  notice={notice}
                  refresh={refresh}
                  pageAlive={() => alive}
                />
                <AdminPager
                  pagination={pagination}
                  page={current().page.page}
                  pageSize={current().page.page_size}
                  total={current().page.total}
                  label={t("page-alerts-title")}
                  onPageChange={(nextPage) => {
                    const next = { ...queryState(), page: BigInt(nextPage) };
                    query.push(next);
                  }}
                />
              </>
            )}
          </Match>
        </Switch>
        <WebhookTable refresh={refresh} />
      </section>
    </PendingCommandsContext>
  );
}
