// 最近 webhook 投递投影；读取失败只提示不可用，不触发重试或修改投递状态。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err } from "@auction/web-shared/result";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import { type AdminAlertWebhook, fetchAdminWebhooks } from "../../api/alerts";
import { useAdminSessionHost } from "../auth/shell";

type WebhookView =
  | { readonly kind: "loading" }
  | { readonly kind: "unavailable" }
  | { readonly kind: "empty" }
  | { readonly kind: "entries"; readonly entries: readonly AdminAlertWebhook[] };

export function WebhookTable(props: { readonly refresh: Cell<bigint> }): JSX.Element {
  const host = useAdminSessionHost();
  const webhooks = createTaskResource(
    () => props.refresh(),
    async (_generation, context) => {
      try {
        return await fetchAdminWebhooks(host, 50, context.signal);
      } catch {
        // 读取异常按稳定传输失败收尾，页面仍显示「Webhook 不可用」。
        return err("network_error");
      }
    },
  );
  const view = createValueMemo((): WebhookView => {
    const current = webhooks();
    if (current === undefined) {
      return { kind: "loading" };
    }
    if (!current.ok) {
      return { kind: "unavailable" };
    }
    return current.value.length === 0
      ? { kind: "empty" }
      : { kind: "entries", entries: current.value };
  });
  const entriesView = createValueMemo(
    (): { readonly entries: readonly AdminAlertWebhook[] } | null => {
      const current = view();
      return current.kind === "entries" ? current : null;
    },
  );
  const emptyView = createValueMemo((): { readonly kind: "empty" } | null => {
    const current = view();
    return current.kind === "empty" ? current : null;
  });
  const unavailableView = createValueMemo((): { readonly kind: "unavailable" } | null => {
    const current = view();
    return current.kind === "unavailable" ? current : null;
  });
  const loadingView = createValueMemo((): { readonly kind: "loading" } | null => {
    const current = view();
    return current.kind === "loading" ? current : null;
  });
  return (
    <Switch>
      <Match when={loadingView()}>
        <p class="admin-panel__note">{t("alert-webhooks-loading")}</p>
      </Match>
      <Match when={unavailableView()}>
        <p class="admin-panel__note">{t("alert-webhooks-unavailable")}</p>
      </Match>
      <Match when={emptyView()}>
        <section>
          <h3>{t("alert-webhooks-title")}</h3>
          <p class="admin-panel__note">{t("alert-webhooks-empty")}</p>
        </section>
      </Match>
      <Match when={entriesView()}>
        {(current) => (
          <section>
            <h3>{t("alert-webhooks-title")}</h3>
            <div class="admin-shell__table-wrap">
              <table class="admin-shell__table" aria-label={t("alert-webhooks-title")}>
                <thead>
                  <tr>
                    <th scope="col">{t("alert-webhooks-status")}</th>
                    <th scope="col">{t("alert-webhooks-kind")}</th>
                    <th scope="col">{t("alert-webhooks-attempts")}</th>
                    <th scope="col">{t("alert-webhooks-next")}</th>
                    <th scope="col">{t("alert-webhooks-failure")}</th>
                    <th scope="col">{t("alert-webhooks-created")}</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={current().entries} keyed={(entry) => entry.id}>
                    {(entry) => (
                      <tr>
                        <td>{webhookStatusLabel(entry().status)}</td>
                        <td>{`${entry().kind} / ${entry().source} / ${entry().scope}`}</td>
                        <td>{entry().attempt_count}</td>
                        <td>
                          <Show when={present(entry().next_attempt_at)}>
                            {(next) => <Time value={next().value} />}
                          </Show>
                        </td>
                        <td>{entry().failure_code ?? ""}</td>
                        <td>
                          <Time value={entry().created_at} />
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </section>
        )}
      </Match>
    </Switch>
  );
}

/** Webhook 状态是数据库闭集；界面复用投递状态译文，未知值保留原码供诊断。 */
function webhookStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return t("delivery-status-pending");
    case "sending":
      return t("delivery-status-sending");
    case "succeeded":
      return t("delivery-status-succeeded");
    case "failed":
      return t("delivery-status-failed");
    case "manual_reconcile":
      return t("delivery-status-manual");
    default:
      return status;
  }
}

/** `Option<T>` 的非空视图；避免把空字符串当成缺失值。 */
function present<T>(value: T | null): { readonly value: T } | null {
  return value === null ? null : { value };
}
