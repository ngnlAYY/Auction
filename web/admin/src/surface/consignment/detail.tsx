// 私有挂单详情；按挂单身份和最近的读取世代显示快照及运营操作。

import type { LossProofReceipt } from "@auction/contracts/dto/admin/consignment/loss";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { Link, type RoutePageProps, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, onCleanup, Show, Switch, untrack } from "solid-js";

import { fetchAdminListingV2 } from "../../api/consignment/read-v2";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { DelistAction, type DelistFeedback } from "./actions";
import { ListingDetailSections } from "./detail-sections";
import { lossCasePath, PositionLossEntry } from "./position-loss";
import { consignmentErrorMessage } from "./presentation";
import { ReleaseHoldAction } from "./release-hold";

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
  const feedback = createCell<DelistFeedback | null>(null);
  // 灭失证据回执与跳转由详情页持有：命令了结后的重读会重建子组件。
  const lossProof = createCell<LossProofReceipt | null>(null);
  const router = useRouter();
  let mounted = true;
  onCleanup(() => {
    mounted = false;
  });
  const detail = createTaskResource(
    () => refresh(),
    async (generation, context) => ({
      generation,
      result: await fetchAdminListingV2(host, listingId, context.signal),
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
          const disposition = () => {
            const current = notice();
            return current.kind === "success" ? current.disposition : null;
          };
          return (
            <Show
              when={error()}
              fallback={
                <p role="status">
                  {disposition() === "hold_set"
                    ? t("consignment-hold-set")
                    : disposition() === "hold_released"
                      ? t("consignment-hold-released")
                      : t("consignment-closed")}
                </p>
              }
            >
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
              <ReleaseHoldAction
                record={current().record}
                commands={commands}
                currentRead={() => projection() !== null}
                onFeedback={(result) => feedback.set(result)}
              />
              <Show when={current().record.position}>
                {(position) => (
                  <PositionLossEntry
                    listingId={listingId}
                    position={position()}
                    proof={lossProof}
                    commands={commands}
                    currentRead={() => projection() !== null}
                    onFeedback={(result) => feedback.set(result)}
                    onConfirmed={(positionId) => {
                      if (mounted) router.push(lossCasePath(positionId));
                    }}
                  />
                )}
              </Show>
            </>
          )}
        </Match>
      </Switch>
    </section>
  );
}
