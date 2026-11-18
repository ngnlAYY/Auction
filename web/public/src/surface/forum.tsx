// Public 论坛同步摘要；正文和诊断细节由 Admin 展示。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { sleepWhileVisible } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show, useContext } from "solid-js";

import { type ForumThread, fetchForumSyncEnabled, fetchForumThread } from "../api/forum";
import { PublicRefreshGenerationContext } from "./board/refresh";
import { WorldTabs, worldLabel } from "./board/world";
import { formatAuctionTime } from "./catalog/presentation";
import { useContentLoading } from "./loading";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  SelectedWorldContext,
} from "./shell/contexts";

/** 先读取独立的开关投影，明确启用后才挂载当前拍卖的论坛进度。 */
export function page(): JSX.Element {
  const statusGeneration = createCell(0);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const visibility = useContext(PageVisibilityContext);
  const syncEnabled = createTaskResource(
    () => ({ status: statusGeneration(), refresh: refreshGeneration() }),
    async (_dependencies, context) => {
      const result = await fetchForumSyncEnabled(context.signal);
      if (!context.alive()) {
        return { ok: false as const, error: "network_error" };
      }
      return result;
    },
  );
  // 关闭和短暂不可用时也定期重读，以便 Worker 重启后自动恢复，不能继续显示旧楼层。
  createTaskResource(
    () => statusGeneration(),
    async (generation, context) => {
      await sleepWhileVisible(visibility, 30_000, context.signal);
      if (!context.alive()) {
        return null;
      }
      statusGeneration.set(Math.min(generation + 1, Number.MAX_SAFE_INTEGER));
      return null;
    },
  );
  useContentLoading(() => syncEnabled() === undefined);
  const syncError = createValueMemo(() => syncEnabled()?.ok === false);
  const syncDisabled = createValueMemo(() => {
    const snapshot = syncEnabled();
    return snapshot?.ok === true && snapshot.value === false;
  });
  const syncReady = createValueMemo(() => {
    const snapshot = syncEnabled();
    return snapshot?.ok === true && snapshot.value === true;
  });

  return (
    <section class="public-forum-page" aria-label={t("page-forum-title")}>
      {/* 世界切换：页面内的按钮组（Solid 版专属；Rust 版用顶栏与页面内的下拉框）。 */}
      <div class="public-world-toolbar">
        <WorldTabs />
      </div>
      <Show when={syncEnabled() !== undefined} fallback={null}>
        <Show when={!syncError()} fallback={forumPageState(() => t("forum-unavailable"), true)}>
          <Show
            when={!syncDisabled()}
            fallback={forumPageState(() => t("forum-sync-disabled"), false)}
          >
            <Show when={syncReady()}>
              <ForumCurrentAuction />
            </Show>
          </Show>
        </Show>
      </Show>
    </section>
  );
}

function ForumCurrentAuction(): JSX.Element {
  const selectedWorld = useContext(SelectedWorldContext);
  const currentAuction = useContext(CurrentAuctionContext);
  const current = createValueMemo(() => {
    const loaded = currentAuction();
    if (loaded === undefined || loaded[0] !== selectedWorld()) {
      return null;
    }
    return loaded[1];
  });
  useContentLoading(() => current() === null);
  const error = createValueMemo(() => current()?.ok === false);
  const noAuction = createValueMemo(() => {
    const result = current();
    return result?.ok === true && result.value === null;
  });
  const closedAuction = createValueMemo(() => {
    const result = current();
    if (result?.ok !== true || result.value === null) {
      return null;
    }
    return result.value.presentation_mode !== "live" ? result.value : null;
  });
  const liveAuction = createValueMemo(() => {
    const result = current();
    if (result?.ok !== true || result.value === null) {
      return null;
    }
    return result.value.presentation_mode === "live" ? result.value : null;
  });

  return (
    <Show when={current() !== null} fallback={null}>
      <Show when={!error()} fallback={forumPageState(() => t("forum-unavailable-note"), true)}>
        <Show
          when={!noAuction()}
          fallback={
            <section class="public-forum-state" role="status" aria-live="polite">
              <h2>{t("forum-no-auction")}</h2>
              <p>{t("forum-no-auction-note")}</p>
            </section>
          }
        >
          <Show
            when={closedAuction()}
            fallback={
              <Show when={liveAuction()}>
                {(auction) => <ForumAuctionPanel auction={auction()} />}
              </Show>
            }
          >
            {(auction) => (
              <article class="public-forum-card">
                <ForumAuctionHeading auction={auction()} />
                <section class="public-forum-state" role="status" aria-live="polite">
                  <h3>{t("forum-not-open")}</h3>
                  <p>{t("forum-not-open-note")}</p>
                </section>
              </article>
            )}
          </Show>
        </Show>
      </Show>
    </Show>
  );
}

function forumPageState(message: () => string, alert: boolean): JSX.Element {
  return (
    <section class="public-forum-state" role={alert ? "alert" : "status"} aria-live="polite">
      <p>{message()}</p>
    </section>
  );
}

function ForumAuctionPanel(props: { readonly auction: CurrentAuctionItemResponse }): JSX.Element {
  const pollGeneration = createCell(0);
  const visibility = useContext(PageVisibilityContext);
  // 拍卖 UUID 是资源依赖；同一场次的标题或状态更新不应重挂载线程读取或重置轮询。
  const auctionId = createValueMemo(() => props.auction.id);
  // 此组件只拥有读取和轮询任务；拍卖 UUID 是显式依赖，不用静态 key 假装重挂载。
  const thread = createTaskResource(
    () => ({ auctionId: auctionId(), pollGeneration: pollGeneration() }),
    async (dependencies, context) => {
      const result = await fetchForumThread(dependencies.auctionId, context.signal);
      if (!context.alive()) {
        return null;
      }
      return [dependencies.auctionId, result] as const;
    },
  );
  // 每次读取完成后才开始轮询间隔，不能用两秒节拍取消尚未完成的慢请求。
  // 发布前两秒、发布后三十秒；明确失败时暂停，重新进入页面后重读。手动刷新入口只在 Admin。
  createTaskResource(
    () => {
      const loadedId = auctionId();
      const state = thread.state();
      const loaded = thread();
      let pollDelayMillis: number | null = null;
      if (state === "ready" && loaded !== undefined && loaded !== null) {
        const [responseId, result] = loaded;
        if (responseId === loadedId && result.ok) {
          if (result.value.status === "published") {
            pollDelayMillis = 30_000;
          } else if (result.value.status !== "failed") {
            pollDelayMillis = 2_000;
          }
        }
      }
      return { pollDelayMillis };
    },
    async (dependencies, context) => {
      const pollDelayMillis = dependencies.pollDelayMillis;
      if (pollDelayMillis === null) {
        return null;
      }
      await sleepWhileVisible(visibility, pollDelayMillis, context.signal);
      if (!context.alive()) {
        return null;
      }
      const nextGeneration = Math.min(pollGeneration.peek() + 1, Number.MAX_SAFE_INTEGER);
      pollGeneration.set(nextGeneration);
      return null;
    },
  );
  // Resource 会保留旧结果；切换场次后不能把旧成功或错误显示在新标题下面。
  const threadState = createValueMemo(() => {
    const loaded = thread();
    if (loaded === undefined || loaded === null || loaded[0] !== auctionId()) {
      return null;
    }
    return loaded[1];
  });
  useContentLoading(() => threadState() === null);
  const threadError = createValueMemo(() => threadState()?.ok === false);
  const threadDetails = createValueMemo(() => {
    const result = threadState();
    return result?.ok === true ? result.value : null;
  });

  return (
    <article class="public-forum-card">
      <ForumAuctionHeading auction={props.auction} />
      <Show when={threadState() !== null} fallback={null}>
        <Show
          when={!threadError()}
          fallback={forumPageState(() => t("forum-unavailable-note"), true)}
        >
          <Show when={threadDetails()}>
            {(details) => <ForumThreadDetails thread={details()} />}
          </Show>
        </Show>
      </Show>
    </article>
  );
}

function ForumAuctionHeading(props: { readonly auction: CurrentAuctionItemResponse }): JSX.Element {
  return (
    <header class="public-forum-card__header">
      <div>
        <h2>{t("forum-current-auction", { number: props.auction.number })}</h2>
        <p class="public-forum-card__world">
          {t("forum-world")}
          {": "}
          {worldLabel(props.auction.world)}
        </p>
      </div>
    </header>
  );
}

function ForumThreadDetails(props: { readonly thread: ForumThread }): JSX.Element {
  return (
    <div class="public-forum-card__content">
      <div class="public-forum-card__status-line">
        {/* biome-ignore lint/a11y/useAriaPropsSupportedByRole: 源码在无 role 的 span 上保留 aria-label。 */}
        <span
          class={["public-forum-status", forumStatusClass(props.thread.status)]}
          aria-label={`${t("forum-status")}: ${forumStatusLabel(props.thread.status)}`}
        >
          {forumStatusLabel(props.thread.status)}
        </span>
        <p role="status" aria-live="polite">
          {forumStatusMessage(props.thread.status)}
        </p>
        <Show when={props.thread.status === "published" ? props.thread.thread_url : null}>
          {(threadUrl) => (
            <a
              class="public-forum-card__link"
              href={threadUrl()}
              target="_blank"
              rel="noopener noreferrer"
            >
              {t("forum-link")}
            </a>
          )}
        </Show>
      </div>
      <Show when={props.thread.content_last_error_code != null}>
        <p class="public-forum-delay" role="alert">
          {t("forum-sync-delayed")}
        </p>
      </Show>
      <dl class="public-forum-metadata">
        <div>
          <dt>{t("forum-progress")}</dt>
          <dd>
            {t("forum-updated-through", {
              post_number: props.thread.processed_through_post_number,
            })}
          </dd>
        </div>
        <div>
          <dt>{t("forum-updated-at")}</dt>
          <dd>
            <time datetime={props.thread.updated_at}>
              {formatAuctionTime(props.thread.updated_at)}
            </time>
          </dd>
        </div>
      </dl>
    </div>
  );
}

function forumStatusLabel(status: string): string {
  switch (status) {
    case "pending":
      return t("forum-state-pending");
    case "publishing":
      return t("forum-state-publishing");
    case "failed":
      return t("forum-state-failed");
    case "published":
      return t("forum-state-published");
    default:
      return t("forum-unavailable");
  }
}

function forumStatusMessage(status: string): string {
  switch (status) {
    case "pending":
      return t("forum-syncing-pending");
    case "publishing":
      return t("forum-syncing-publishing");
    case "failed":
      return t("forum-syncing-failed");
    case "published":
      return t("forum-published");
    default:
      return t("forum-unavailable");
  }
}

function forumStatusClass(status: string): string {
  switch (status) {
    case "published":
      return "public-forum-status--published";
    case "failed":
      return "public-forum-status--failed";
    case "publishing":
      return "public-forum-status--publishing";
    default:
      return "public-forum-status--pending";
  }
}
