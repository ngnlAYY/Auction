// Admin realtime 运维页面状态与 replay 对话框。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, onCleanup, Show, Switch } from "solid-js";

import {
  fetchRealtimeEventPage,
  fetchRealtimeEventTotal,
  fetchRealtimeFailurePage,
  fetchRealtimeLagPage,
  type RealtimeFailure,
  type RealtimeFailurePage,
} from "../../api/realtime";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPageWithState, useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import { AutoPollControls, useAutoPoll } from "../poll";
import {
  type AdminActionFeedback,
  adminActionFeedbackIsSuccess,
  adminActionFeedbackMessage,
  adminApiErrorMessage,
} from "../presentation";
import {
  realtimeQueryEventPageRequest,
  realtimeQueryFailurePageRequest,
  realtimeQueryWithEventFilters,
  realtimeQueryWithFailureFilters,
  useRealtimeQueryState,
} from "../query/realtime";
import { RealtimeEventTable } from "./event";
import { RealtimeFailureTable } from "./failure";
import { fetchFiltered, filterValueFromText } from "./filter-field";
import {
  RealtimeEventFilters,
  RealtimeFailureFilters,
  RealtimeFailureStateFilter,
} from "./filters";
import { RealtimeLagTable } from "./lag";
import {
  activityCountState,
  consumerCountState,
  failureCountState,
  metricCard,
  metricValue,
} from "./overview";
import { bumpRefresh, paginationPage, useSectionReads } from "./page-support";
import { RealtimeReplayDialog } from "./replay-dialog";

/** Outbox 路由展示已持久化事件元数据与 realtime failure/lag/replay 安全投影；未冻结的
 * 外部广播 sink 不在页面中虚构状态或成功结果。 */
export function outboxPage(): JSX.Element {
  return pageWithCopy();
}

function pageWithCopy(): JSX.Element {
  const host = useAdminSessionHost();
  const eventRefresh = createCell(0n);
  const failureRefresh = createCell(0n);
  const lagRefresh = createCell(0n);
  const notice = createCell<AdminActionFeedback | null>(null);
  const selectedFailure = createCell<string | null>(null);
  const replayRefresh = createCommandReload(0n);
  const pending = usePendingCommands(replayRefresh, (entity) =>
    entity.startsWith("realtime-failure:"),
  );
  const schemaVersion = createCell("1");
  const publicPayload = createCell("{}");
  const replayInvalid = createCell(false);
  const replayBusy = (): boolean => pending.anyBusy();
  let alive = true;
  onCleanup(() => {
    alive = false;
  });

  // 已应用的事件/failure 筛选与三个分区的分页跟地址栏双向同步；筛选输入框草稿（留在
  // 筛选组件本地）、replay 对话框与负载输入是瞬态，不入 URL。
  const realtimeQuery = useRealtimeQueryState();
  const query = realtimeQuery.location;
  const queryState = query.state;
  const eventPagination = useAdminPagination(
    "realtime-events",
    ["event_page_size"],
    200,
    1_000_000,
  );
  const failurePagination = useAdminPagination(
    "realtime-failures",
    ["failure_page_size"],
    200,
    1_000_000,
  );
  const lagPagination = useAdminPagination("realtime-lag", ["lag_page_size"], 200, 1_000_000);

  // 三个分区只订阅自己的请求身份；另一区翻页或切换轮询间隔不重发无关 GET。
  const eventQuery = createValueMemo(() => realtimeQueryEventPageRequest(queryState()));
  const lagQuery = createValueMemo(() => queryState().lagPage);
  const failureQuery = createValueMemo(() => realtimeQueryFailurePageRequest(queryState()));
  const eventAllowed = createValueMemo(() => eventQuery() !== null);
  const failureAllowed = createValueMemo(() => failureQuery() !== null);

  const [events, eventsStatus] = useAdminPageWithState(
    eventPagination,
    () => {
      replayRefresh();
      eventRefresh();
      return eventQuery();
    },
    (request, size, signal) =>
      fetchFiltered(request, (current) => {
        const page = paginationPage(current[0]);
        return page === null
          ? Promise.resolve(err("admin_pagination_bounds"))
          : fetchRealtimeEventPage(host, page, current[1], current[2], size, signal);
      }),
  );
  const eventOverview = createTaskResource(
    () => {
      replayRefresh();
      eventRefresh();
      return eventAllowed();
    },
    async (allowed, context) =>
      fetchFiltered(allowed ? true : null, () => fetchRealtimeEventTotal(host, context.signal)),
  );
  const [lag, lagStatus] = useAdminPageWithState(
    lagPagination,
    () => {
      replayRefresh();
      lagRefresh();
      return lagQuery();
    },
    (page, size, signal) => {
      const pageNumber = paginationPage(page);
      return pageNumber === null
        ? Promise.resolve(err("admin_pagination_bounds"))
        : fetchRealtimeLagPage(host, pageNumber, "", "", size, signal);
    },
  );
  const [failures, failuresStatus] = useAdminPageWithState(
    failurePagination,
    () => {
      replayRefresh();
      failureRefresh();
      return failureQuery();
    },
    (request, size, signal) =>
      fetchFiltered(request, (current) => {
        const page = paginationPage(current[0]);
        return page === null
          ? Promise.resolve(err("admin_pagination_bounds"))
          : fetchRealtimeFailurePage(
              host,
              page,
              current[1],
              current[2],
              current[3],
              current[4],
              size,
              signal,
            );
      }),
  );
  const unresolvedFailures = createTaskResource(
    () => {
      replayRefresh();
      failureRefresh();
      return failureAllowed();
    },
    async (allowed, context) =>
      fetchFiltered(allowed ? true : null, () =>
        fetchRealtimeFailurePage(host, 1, "", "", "", "dead_letter", 10, context.signal),
      ),
  );

  // 各分区只在自己的读取（含配套计数）完成后轮询；慢请求不能被下一节拍取消。
  const pollSeconds = createValueMemo(() => BigInt(queryState().pollSeconds));
  useAutoPoll(pollSeconds, () => {
    if (eventAllowed() && eventsStatus() === "ready" && eventOverview.state() === "ready") {
      bumpRefresh(eventRefresh);
    }
    if (lagStatus() === "ready") {
      bumpRefresh(lagRefresh);
    }
    if (
      failureAllowed() &&
      failuresStatus() === "ready" &&
      unresolvedFailures.state() === "ready"
    ) {
      bumpRefresh(failureRefresh);
    }
  });

  // 应用草稿筛选并回到第一页；事件分区资源响应查询状态变化自动重新加载。
  const loadEvents = (filters: readonly [string, string]): void => {
    query.push(realtimeQueryWithEventFilters(queryState(), filters));
  };
  const loadFailures = (filters: readonly [string, string, string]): void => {
    query.push(realtimeQueryWithFailureFilters(queryState(), filters));
    selectedFailure.set(null);
  };

  const { page: eventPage, error: eventError } = useSectionReads(events);
  const { page: lagPage, error: lagError } = useSectionReads(lag);
  const { page: failurePage, error: failureError } = useSectionReads(failures);
  const unresolvedFailurePage = createValueMemo((): RealtimeFailurePage | null => {
    const current = unresolvedFailures();
    if (!failureAllowed() || current === undefined || !current.ok) {
      return null;
    }
    return current.value;
  });
  const eventTotal = createValueMemo((): number | null => {
    const current = eventOverview();
    if (!eventAllowed() || current === undefined || !current.ok) {
      return null;
    }
    return current.value;
  });
  const consumerTotal = createValueMemo((): number | null => {
    const current = lagPage();
    return current === null ? null : current.pagination.total;
  });
  const failureTotal = createValueMemo((): number | null => {
    const current = unresolvedFailurePage();
    return current === null ? null : current.pagination.total;
  });
  // 直接借用已完成快照；选择只在当前 failure 页的快照里解析。
  const selected = createValueMemo((): RealtimeFailure | null => {
    const id = selectedFailure();
    const page = failurePage();
    if (!failureAllowed() || id === null || page === null) {
      return null;
    }
    return page.failures.find((failure) => failure.id === id) ?? null;
  });

  const refreshAll = (): void => {
    selectedFailure.set(null);
    notice.set(null);
    // 手动刷新把三个分区都带回第一页并写回地址栏。
    const next = {
      ...queryState(),
      eventPage: 1n,
      lagPage: 1n,
      failurePage: 1n,
    };
    query.push(next);
    bumpRefresh(eventRefresh);
    bumpRefresh(lagRefresh);
    bumpRefresh(failureRefresh);
  };

  const selectFailure = (failure: RealtimeFailure): void => {
    if (replayBusy() || pending.contains(`realtime-failure:${failure.id}`)) {
      return;
    }
    schemaVersion.set("1");
    publicPayload.set("{}");
    replayInvalid.set(false);
    selectedFailure.set(failure.id);
    notice.set(null);
  };

  return (
    <PendingCommandsContext value={pending}>
      <section class="admin-realtime-page" aria-labelledby="outbox-title">
        <PendingCommandPanel owner={pending} />
        <div class="admin-realtime-heading">
          <h1 id="outbox-title">{t("page-outbox-title")}</h1>
          <div class="admin-realtime-heading__actions">
            <AutoPollControls
              pollSeconds={pollSeconds()}
              onChange={(seconds) => {
                // 开关与间隔变更立即生效并写回地址栏。
                query.push({ ...queryState(), pollSeconds: Number(seconds) });
              }}
            />
            <button type="button" class="admin-btn-secondary" onClick={refreshAll}>
              {t("realtime-refresh")}
            </button>
          </div>
        </div>
        <p class="admin-realtime-intro">{t("page-outbox-note")}</p>
        {/* biome-ignore lint/a11y/useSemanticElements: 源码以 div + role 保留区域，不能改成 section。 */}
        <div
          class="admin-realtime-overview"
          role="region"
          aria-label={t("realtime-overview-title")}
        >
          {metricCard(
            t("realtime-overview-events"),
            metricValue(eventTotal()),
            activityCountState(eventTotal()),
          )}
          {metricCard(
            t("realtime-overview-consumers"),
            metricValue(consumerTotal()),
            consumerCountState(consumerTotal()),
          )}
          {metricCard(
            t("realtime-overview-failures"),
            metricValue(failureTotal()),
            failureCountState(failureTotal()),
          )}
        </div>

        <section class="admin-realtime-section" aria-labelledby="realtime-events-title">
          <div class="admin-realtime-section__header">
            <div>
              <h2 id="realtime-events-title">{t("realtime-event-title")}</h2>
              <p>{t("realtime-event-note")}</p>
            </div>
          </div>
          <RealtimeEventFilters
            applied={realtimeQuery.eventFilters}
            invalidApplied={!eventAllowed()}
            onLoad={loadEvents}
          />
          <Show when={eventAllowed()}>
            <Switch
              fallback={<AsyncState kind="loading" title={t("realtime-loading")} detail="" />}
            >
              <Match when={eventError()}>
                {(error) => (
                  <AsyncState
                    kind="error"
                    title={adminApiErrorMessage(error().code)}
                    detail=""
                    // 错误态提供页内重试入口：仅重载事件分区（含总览计数）。
                    retryLabel={t("admin-retry")}
                    onRetry={() => bumpRefresh(eventRefresh)}
                  />
                )}
              </Match>
              <Match when={eventPage()}>{(page) => <RealtimeEventTable page={page()} />}</Match>
            </Switch>
            <Show when={eventPage()}>
              {(page) => (
                <AdminPager
                  pagination={eventPagination}
                  page={page().pagination.page}
                  pageSize={page().pagination.page_size}
                  total={page().pagination.total}
                  label={t("realtime-event-title")}
                  onPageChange={(nextPage) => {
                    const next = { ...queryState(), eventPage: BigInt(nextPage) };
                    query.push(next);
                  }}
                />
              )}
            </Show>
          </Show>
        </section>

        <section class="admin-realtime-section" aria-labelledby="realtime-lag-title">
          <div class="admin-realtime-section__header">
            <div>
              <h2 id="realtime-lag-title">{t("realtime-lag-title")}</h2>
              <p>{t("realtime-lag-note")}</p>
            </div>
          </div>
          <Switch fallback={<AsyncState kind="loading" title={t("realtime-loading")} detail="" />}>
            <Match when={lagError()}>
              {(error) => (
                <AsyncState
                  kind="error"
                  title={adminApiErrorMessage(error().code)}
                  detail=""
                  // 错误态提供页内重试入口：仅重载 lag 分区。
                  retryLabel={t("admin-retry")}
                  onRetry={() => bumpRefresh(lagRefresh)}
                />
              )}
            </Match>
            <Match when={lagPage()}>
              {(page) => (
                <>
                  <RealtimeLagTable page={page()} />
                  <AdminPager
                    pagination={lagPagination}
                    page={page().pagination.page}
                    pageSize={page().pagination.page_size}
                    total={page().pagination.total}
                    label={t("realtime-lag-title")}
                    onPageChange={(nextPage) => {
                      query.push({ ...queryState(), lagPage: BigInt(nextPage) });
                    }}
                  />
                </>
              )}
            </Match>
          </Switch>
        </section>

        <section class="admin-realtime-section" aria-labelledby="realtime-failures-title">
          <div class="admin-realtime-section__header">
            <div>
              <h2 id="realtime-failures-title">{t("realtime-failure-title")}</h2>
              <p>{t("realtime-failure-note")}</p>
            </div>
            <RealtimeFailureStateFilter
              value={queryState().failureState}
              onChange={(value: string) => {
                query.push({
                  ...queryState(),
                  failureState: filterValueFromText(value),
                  failurePage: 1n,
                });
                selectedFailure.set(null);
              }}
            />
          </div>
          <Show when={failureAllowed()}>
            <Switch
              fallback={<AsyncState kind="loading" title={t("realtime-loading")} detail="" />}
            >
              <Match when={failureError()}>
                {(error) => (
                  <AsyncState
                    kind="error"
                    title={adminApiErrorMessage(error().code)}
                    detail=""
                    // 错误态提供页内重试入口：仅重载 failure 分区（含未解决计数）。
                    retryLabel={t("admin-retry")}
                    onRetry={() => bumpRefresh(failureRefresh)}
                  />
                )}
              </Match>
              <Match when={failurePage()}>
                {(page) => <RealtimeFailureTable page={page()} onSelect={selectFailure} />}
              </Match>
            </Switch>
          </Show>
          <Show when={notice()}>
            {(feedback) => (
              <p
                class={
                  adminActionFeedbackIsSuccess(feedback())
                    ? "admin-panel__note action-feedback action-feedback--success"
                    : "admin-panel__note action-feedback action-feedback--error"
                }
                role={adminActionFeedbackIsSuccess(feedback()) ? "status" : "alert"}
              >
                {adminActionFeedbackMessage(feedback(), t("realtime-replay-success"))}
              </p>
            )}
          </Show>
          <Show when={failureAllowed()}>
            <Show when={failurePage()}>
              {(page) => (
                <AdminPager
                  pagination={failurePagination}
                  page={page().pagination.page}
                  pageSize={page().pagination.page_size}
                  total={page().pagination.total}
                  label={t("realtime-failure-title")}
                  onPageChange={(nextPage) => {
                    query.push({ ...queryState(), failurePage: BigInt(nextPage) });
                  }}
                />
              )}
            </Show>
          </Show>
          <RealtimeFailureFilters
            applied={realtimeQuery.failureFilters}
            invalidApplied={!failureAllowed()}
            onLoad={loadFailures}
          />
        </section>

        <Show when={selected()}>
          {(failure) => (
            <RealtimeReplayDialog
              failure={failure()}
              host={host}
              pending={pending}
              notice={notice}
              schemaVersion={schemaVersion}
              publicPayload={publicPayload}
              replayInvalid={replayInvalid}
              busy={replayBusy}
              pageAlive={() => alive}
              onClose={() => selectedFailure.set(null)}
              onSubmitted={() => selectedFailure.set(null)}
            />
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}
