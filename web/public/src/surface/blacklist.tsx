// Public 黑名单页面。

import type {
  PublicBlacklistEntryResponse,
  PublicBlacklistResponse,
} from "@auction/contracts/dto/public/account";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import { fetchBlacklist } from "../api/blacklist";
import { PublicRefreshGenerationContext } from "./board/refresh";
import { useContentLoading } from "./loading";
import { PublicPager, publicPageMatches, updatePublicPageSize } from "./pagination";
import { updatePublicQueryState, usePublicQueryState } from "./query";

type BlacklistRequest = readonly [page: number, pageSize: number];

interface LoadedBlacklist {
  readonly requested: BlacklistRequest;
  readonly result: Result<PublicBlacklistResponse, string>;
}

/** Public 黑名单分页页；没有当前页码结果时由顶栏提示加载，正文留空。 */
export function page(): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  // 页码与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const requested = createValueMemo(() => [pageNumber(), pageSize()] as const);
  const blacklist = createTaskResource(
    () => ({ generation: refreshGeneration(), requested: requested() }),
    async (dependencies, context): Promise<LoadedBlacklist | null> => {
      const [page, size] = dependencies.requested;
      const result = await fetchBlacklist(page, size, context.signal);
      if (!context.alive()) {
        return null;
      }
      return { requested: dependencies.requested, result };
    },
  );
  // 查询变化时旧快照不能参与列表展示或页码纠偏，同查询后台刷新仍保留旧值。
  const current = createValueMemo((): Result<PublicBlacklistResponse, string> | null => {
    const loaded = blacklist();
    if (loaded === null || loaded === undefined) {
      return null;
    }
    const [page, size] = requested();
    const currentRequest = loaded.requested;
    if (currentRequest[0] !== page || currentRequest[1] !== size) {
      return null;
    }
    return loaded.result;
  });
  const currentPage = createValueMemo(() => {
    const result = current();
    if (result === null || !result.ok) {
      return null;
    }
    const page = result.value;
    return publicPageMatches(page.page, page.page_size, pageNumber(), pageSize()) ? page : null;
  });
  // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
  useContentLoading(() => {
    const result = current();
    if (result === null) {
      return true;
    }
    return result.ok ? currentPage() === null : false;
  });
  const retry = (): void => {
    const generation = refreshGeneration.peek();
    refreshGeneration.set(Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
  };

  return (
    <Switch>
      <Match when={current() === null}>{null}</Match>
      <Match when={current()?.ok === false}>
        <AsyncState
          kind="error"
          title={t("page-blacklist-title")}
          detail={t("data-error")}
          retryLabel={t("data-retry")}
          onRetry={retry}
        />
      </Match>
      <Match when={currentPage()}>
        {(page) => (
          <BlacklistTable
            page={page()}
            onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
            onPageChange={(nextPage) => {
              updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.page = nextPage;
                },
                "push",
              );
            }}
            onPageClamp={(nextPage) => {
              updatePublicQueryState(
                queryStateSignal,
                (query) => {
                  query.page = nextPage;
                },
                "replace",
              );
            }}
          />
        )}
      </Match>
    </Switch>
  );
}

function BlacklistTable(props: {
  readonly page: PublicBlacklistResponse;
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp: (page: number) => void;
  readonly onPageSizeChange: (size: number) => void;
}): JSX.Element {
  return (
    <section class="public-blacklist-page" aria-label={t("page-blacklist-title")}>
      <Show
        when={props.page.users.length === 0}
        fallback={
          <div class="public-blacklist-table-wrap">
            <table class="public-blacklist-table">
              <TableColumns layout="public-blacklist" widths={[22, 54, 24]} />
              <thead>
                <tr>
                  <th>{t("blacklist-player")}</th>
                  <th>{t("blacklist-reason")}</th>
                  <th>{t("blacklist-time")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={props.page.users} keyed={false}>
                  {(entry) => <BlacklistRow entry={entry()} />}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="public-blacklist-empty">{t("blacklist-empty")}</p>
      </Show>
      {/* 没有任何记录时分页器只会显示“共 0 条”，不渲染；有记录但页码越界时仍需它纠偏。 */}
      <Show when={props.page.total > 0}>
        <PublicPager
          page={props.page.page}
          pageSize={props.page.page_size}
          total={props.page.total}
          hasMore={false}
          label={t("page-blacklist-title")}
          maxOffset={2147483647}
          onPageChange={props.onPageChange}
          onPageClamp={props.onPageClamp}
          onPageSizeChange={props.onPageSizeChange}
        />
      </Show>
    </section>
  );
}

function BlacklistRow(props: { readonly entry: PublicBlacklistEntryResponse }): JSX.Element {
  return (
    <tr>
      <td data-label={t("blacklist-player")}>
        <TruncatedText value={props.entry.username} portal={true} />
      </td>
      <td data-label={t("blacklist-reason")}>
        <TruncatedText value={props.entry.reason} size="wide" portal={true} />
      </td>
      <td data-label={t("blacklist-time")}>
        <Time value={props.entry.blacklisted_at} />
      </td>
    </tr>
  );
}
