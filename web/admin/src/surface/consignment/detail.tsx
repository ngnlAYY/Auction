// 私有挂单详情；按挂单身份和最近的读取世代显示快照及运营操作。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, untrack } from "solid-js";

import { fetchAdminListing } from "../../api/consignment/detail";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { DelistAction } from "./actions";
import { ListingDetailSections } from "./detail-sections";
import { consignmentErrorMessage } from "./presentation";

/** 路由页内切换挂单时卸载旧私有视图、草稿和确认框。 */
export function detailPage(props: RoutePageProps): JSX.Element {
  return (
    <For each={[props.params.listing_id ?? ""]} keyed={(listingId) => listingId}>
      {(listingId) => <Detail listingId={untrack(listingId)} />}
    </For>
  );
}

function Detail(props: { readonly listingId: string }): JSX.Element {
  const host = useAdminSessionHost();
  const listingId = untrack(() => props.listingId);
  const refresh = createCommandReload();
  const commands = usePendingCommands(refresh, (entity) => entity === `consignment:${listingId}`);
  const feedback = createCell<AdminActionFeedback | null>(null);
  const detail = createTaskResource(
    () => refresh(),
    async (generation, context) => ({
      generation,
      result: await fetchAdminListing(host, listingId, context.signal),
    }),
  );
  // 命令了结会推进读取世代。刷新时丢弃旧授权，旧确认框不能走新命令。
  const loaded = createValueMemo(() => {
    const read = detail();
    return read?.generation === refresh() ? read.result : null;
  });
  const projection = createValueMemo(() => {
    const result = loaded();
    return result?.ok ? result.value : null;
  });
  const loadError = createValueMemo(() => {
    const result = loaded();
    return result !== null && !result.ok ? result.error : null;
  });

  return (
    <section class="admin-consignment-detail" aria-labelledby="consignment-detail-title">
      <div class="admin-consignment-detail__header">
        <h1 id="consignment-detail-title" tabindex="-1">
          {t("delivery-detail-open")}
        </h1>
        <Link href="/consignment">{t("consignment-admin-title")}</Link>
        <Show when={projection()}>
          <button type="button" onClick={() => advanceCommandReload(refresh)}>
            {t("admin-record-refresh")}
          </button>
        </Show>
      </div>
      <Show when={feedback()}>
        {(notice) => {
          const error = () => {
            const current = notice();
            return current.kind === "error" ? current.code : null;
          };
          return (
            <Show when={error()} fallback={<p role="status">{t("consignment-closed")}</p>}>
              {(code) => (
                <p role="alert" class="action-feedback action-feedback--error">
                  {consignmentErrorMessage(code())}
                </p>
              )}
            </Show>
          );
        }}
      </Show>
      <PendingCommandPanel owner={commands} />
      <Switch
        fallback={<AsyncState kind="loading" title={t("dashboard-legacy-loading")} detail="" />}
      >
        <Match when={loadError()}>
          {(error) => (
            <AsyncState
              kind="error"
              title={consignmentErrorMessage(error())}
              detail=""
              retryLabel={t("admin-retry")}
              onRetry={() => advanceCommandReload(refresh)}
            />
          )}
        </Match>
        <Match when={projection()}>
          {(current) => (
            <>
              <ListingDetailSections detail={current()} />
              <DelistAction
                record={current().record}
                commands={commands}
                currentRead={() => projection() !== null}
                onFeedback={(result) => feedback.set(result)}
              />
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}
