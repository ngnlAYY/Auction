// 结算退件列表页：按范围、状态、证据分类、义务种类与世界筛选，分页列出案件，进入详情处理。
// 筛选与页码同步到地址栏；列表只读，所有处理动作在详情页确认后提交。

import {
  ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
  ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
  ADMIN_SETTLEMENT_RETURN_STATUSES,
  type AdminSettlementReturnListResponse,
  type AdminSettlementReturnSummaryResponse,
} from "@auction/contracts/dto/admin/returns";
import type { QueryState } from "@auction/web-shared/net/query";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import { validUuid } from "@auction/web-shared/ui/validation";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch } from "solid-js";

import { fetchSettlementReturns } from "../../api/returns";
import { trimRustWhitespace } from "../../api/session-time";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage, adminWorldLabel } from "../presentation";
import { type ReturnsQuery, returnsQueryFilters, useReturnsQueryState } from "../query/returns";
import {
  kindKey,
  label,
  locationKey,
  matchKey,
  reasonKey,
  showsAssetLocation,
  statusKey,
} from "./labels";

type ReturnsView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: AdminSettlementReturnListResponse };

/** 结算退件只读列表的入口；案件处理动作在详情页确认后提交。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const query = useReturnsQueryState();
  const queryState = query.state;
  const refresh = createCommandReload(0n);
  const commands = usePendingCommands(refresh, (entity) => entity.startsWith("settlement-return:"));
  const lookup = createCell("");
  const lookupInvalid = createCell(false);
  const router = useRouter();
  const openLookup = (event: SubmitEvent): void => {
    event.preventDefault();
    const target = trimRustWhitespace(lookup());
    if (!validUuid(target)) {
      lookupInvalid.set(true);
      return;
    }
    lookupInvalid.set(false);
    router.push(`/settlement-returns/${target}`);
  };
  const pagination = useAdminPagination("settlement-returns", ["page_size"], 100, 10_000);
  const returns = useAdminPage(
    pagination,
    () => {
      refresh();
      return queryState();
    },
    (request, size, signal) =>
      fetchSettlementReturns(host, returnsQueryFilters(request), request.page, size, signal),
  );
  const returnsView = createValueMemo((): ReturnsView => {
    const loaded = returns();
    if (loaded === null) {
      return { kind: "loading" };
    }
    return loaded.ok
      ? { kind: "ready", page: loaded.value }
      : { kind: "error", code: loaded.error };
  });
  const readyPage = createValueMemo(() => {
    const view = returnsView();
    return view.kind === "ready" ? view.page : null;
  });
  const loadError = createValueMemo(() => {
    const view = returnsView();
    return view.kind === "error" ? view : null;
  });
  const current = createValueMemo(() => queryState());

  return (
    <PendingCommandsContext value={commands}>
      <section class="admin-settlement-returns" aria-labelledby="settlement-returns-title">
        <h1 id="settlement-returns-title">{t("page-settlement-returns-title")}</h1>
        <p class="admin-panel__note">{t("settlement-return-intro")}</p>
        <form class="admin-settlement-returns__lookup" onSubmit={openLookup}>
          <label for="settlement-return-lookup-id">
            <span>{t("settlement-return-lookup-label")}</span>
            <input
              id="settlement-return-lookup-id"
              type="text"
              value={lookup()}
              onInput={(event) => {
                lookup.set(event.currentTarget.value);
                lookupInvalid.set(false);
              }}
            />
          </label>
          <button type="submit">{t("settlement-return-lookup-open")}</button>
          <Show when={lookupInvalid()}>
            <p class="action-feedback action-feedback--error" role="alert">
              {t("settlement-return-lookup-invalid")}
            </p>
          </Show>
        </form>
        <div class="admin-settlement-returns__filters">
          <label>
            {t("settlement-return-filter-scope")}
            <select
              value={current().all ? "all" : "open"}
              onChange={(event) => {
                pushFilter(query, (next) => {
                  next.all = event.currentTarget.value === "all";
                });
              }}
            >
              <option value="open">{t("settlement-return-scope-open")}</option>
              <option value="all">{t("settlement-return-scope-all")}</option>
            </select>
          </label>
          <label>
            {t("settlement-return-filter-status")}
            <select
              value={current().status}
              onChange={(event) => {
                pushFilter(query, (next) => {
                  next.status = closedChoice(
                    ADMIN_SETTLEMENT_RETURN_STATUSES,
                    event.currentTarget.value,
                  );
                });
              }}
            >
              <option value="">{t("settlement-return-filter-any")}</option>
              <For each={ADMIN_SETTLEMENT_RETURN_STATUSES} keyed={false}>
                {(status) => <option value={status()}>{label(statusKey, status())}</option>}
              </For>
            </select>
          </label>
          <label>
            {t("settlement-return-filter-match")}
            <select
              value={current().matchState}
              onChange={(event) => {
                pushFilter(query, (next) => {
                  next.matchState = closedChoice(
                    ADMIN_SETTLEMENT_RETURN_MATCH_STATES,
                    event.currentTarget.value,
                  );
                });
              }}
            >
              <option value="">{t("settlement-return-filter-any")}</option>
              <For each={ADMIN_SETTLEMENT_RETURN_MATCH_STATES} keyed={false}>
                {(state) => <option value={state()}>{label(matchKey, state())}</option>}
              </For>
            </select>
          </label>
          <label>
            {t("settlement-return-filter-kind")}
            <select
              value={current().obligationKind}
              onChange={(event) => {
                pushFilter(query, (next) => {
                  next.obligationKind = closedChoice(
                    ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS,
                    event.currentTarget.value,
                  );
                });
              }}
            >
              <option value="">{t("settlement-return-filter-any")}</option>
              <For each={ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS} keyed={false}>
                {(kind) => <option value={kind()}>{label(kindKey, kind())}</option>}
              </For>
            </select>
          </label>
          <label>
            {t("settlement-return-filter-world")}
            <select
              value={current().world}
              onChange={(event) => {
                pushFilter(query, (next) => {
                  next.world = closedChoice(["main", "isekai"], event.currentTarget.value);
                });
              }}
            >
              <option value="">{t("settlement-return-filter-any")}</option>
              <option value="main">{adminWorldLabel("main")}</option>
              <option value="isekai">{adminWorldLabel("isekai")}</option>
            </select>
          </label>
        </div>
        <Switch
          fallback={<AsyncState kind="loading" title={t("settlement-return-loading")} detail="" />}
        >
          <Match when={loadError()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(refresh)}
              />
            )}
          </Match>
          <Match when={readyPage()}>
            {(page) => (
              <>
                <ReturnsTable page={page()} />
                <AdminPager
                  pagination={pagination}
                  page={page().pagination.page}
                  pageSize={page().pagination.page_size}
                  total={page().pagination.total}
                  label={t("settlement-return-pagination")}
                  onPageChange={(nextPage) => {
                    query.push({ ...queryState(), page: BigInt(nextPage) });
                  }}
                />
              </>
            )}
          </Match>
        </Switch>
      </section>
    </PendingCommandsContext>
  );
}

/** 应用一项筛选变化并回到第一页，避免落在新结果中不存在的深页。 */
function pushFilter(query: QueryState<ReturnsQuery>, change: (next: ReturnsQuery) => void): void {
  const next = { ...query.state() };
  change(next);
  next.page = 1n;
  query.push(next);
}

/** 下拉框取值只接受闭集；其他值（含空串）回到“不限”。 */
function closedChoice(allowed: readonly string[], value: string): string {
  return allowed.includes(value) ? value : "";
}

function ReturnsTable(props: { readonly page: AdminSettlementReturnListResponse }): JSX.Element {
  return (
    <Show
      when={props.page.returns.length > 0}
      fallback={<AsyncState kind="empty" title={t("settlement-return-empty")} detail="" />}
    >
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-settlement-returns__table"
          aria-label={t("page-settlement-returns-title")}
        >
          <thead>
            <tr>
              <th scope="col">{t("settlement-return-column-received")}</th>
              <th scope="col">{t("settlement-return-column-status")}</th>
              <th scope="col">{t("settlement-return-column-location")}</th>
              <th scope="col">{t("settlement-return-column-kind")}</th>
              <th scope="col">{t("settlement-return-column-recipient")}</th>
              <th scope="col">{t("settlement-return-column-assets")}</th>
              <th scope="col">{t("settlement-return-column-match")}</th>
              <th scope="col">{t("settlement-return-column-world")}</th>
              <th scope="col">{t("settlement-return-column-case")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.page.returns} keyed={(item) => item.return_id}>
              {(item) => <ReturnRow item={item()} />}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}

function ReturnRow(props: { readonly item: AdminSettlementReturnSummaryResponse }): JSX.Element {
  const kind = createValueMemo(() =>
    props.item.obligation_kind === null
      ? t("value-empty")
      : label(kindKey, props.item.obligation_kind),
  );
  const recipient = createValueMemo(() => props.item.recipient ?? t("value-empty"));
  const assets = createValueMemo(() => assetsText(props.item));
  const classification = createValueMemo(
    () =>
      `${label(matchKey, props.item.match_state)} · ${label(reasonKey, props.item.reason_code)}`,
  );
  const shortId = createValueMemo(() => Array.from(props.item.return_id).slice(0, 8).join(""));
  return (
    <tr>
      <td data-label={t("settlement-return-column-received")}>
        <Time value={props.item.received_at} />
      </td>
      <td data-label={t("settlement-return-column-status")}>
        <span class={statusBadge(props.item.status)}>{label(statusKey, props.item.status)}</span>
      </td>
      <td data-label={t("settlement-return-column-location")}>
        {showsAssetLocation(props.item.status)
          ? label(locationKey, props.item.asset_location)
          : t("settlement-return-location-resent-in-transit")}
      </td>
      <td data-label={t("settlement-return-column-kind")}>{kind()}</td>
      <td data-label={t("settlement-return-column-recipient")}>
        <TruncatedText value={recipient()} portal={true} />
      </td>
      <td data-label={t("settlement-return-column-assets")}>{assets()}</td>
      <td data-label={t("settlement-return-column-match")}>
        <TruncatedText value={classification()} size="wide" portal={true} />
      </td>
      <td data-label={t("settlement-return-column-world")}>
        {adminWorldLabel(props.item.world_id)}
      </td>
      <td data-label={t("settlement-return-column-case")}>
        <Link href={`/settlement-returns/${props.item.return_id}`} title={props.item.return_id}>
          {shortId()}
        </Link>
      </td>
    </tr>
  );
}

/** 退回资产摘要：Credits、COD 与附件件数，全部来自服务端冻结证据。 */
export function assetsText(item: AdminSettlementReturnSummaryResponse): string {
  return t("settlement-return-assets-value", {
    credits: String(item.credits),
    cod: String(item.cod),
    attachments: String(item.attachment_count),
  });
}

/** 状态徽标：未结案按是否需要人工核对分级，已结案为中性。 */
function statusBadge(status: string): string {
  switch (status) {
    case "quarantined":
    case "take_unknown":
      return "admin-dashboard-badge admin-dashboard-badge--error";
    case "awaiting_decision":
    case "recheck_pending":
    case "take_pending":
    case "resend_pending":
      return "admin-dashboard-badge admin-dashboard-badge--warn";
    default:
      return "admin-dashboard-badge admin-dashboard-badge--neutral";
  }
}
