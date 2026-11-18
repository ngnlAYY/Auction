//! Public 页面外壳：顶栏、侧栏导航、会话与实时目标的唯一宿主，各页面只消费它提供的上下文。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { usePopstateRevision } from "@auction/web-shared/net/query";
import { useDialogFocusTrap, useRouteFocus } from "@auction/web-shared/platform/a11y";
import { useSecondTicker } from "@auction/web-shared/platform/clock";
import { loadSidebarCollapsed, saveSidebarCollapsed } from "@auction/web-shared/platform/sidebar";
import { sleep } from "@auction/web-shared/platform/timer";
import { createPageVisibility, sleepWhileVisible } from "@auction/web-shared/platform/visibility";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { useRouter } from "@auction/web-shared/router";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import type { JSX } from "@solidjs/web";
import { createEffect, onSettled, useContext } from "solid-js";
import { fetchCurrentAuction } from "../../api/queries/current";
import { fetchPublicReadiness } from "../../api/status";
import { prefetchBoardLots, rememberCurrentAuction } from "../board/prefetch";
import {
  type PublicConnectionState,
  type PublicReadinessState,
  type PublicRealtimePageKey,
  type PublicRealtimeTarget,
  publicConnectionState,
  tickerAuction,
} from "../board/realtime";
import { type PublicBidRefresh, refreshPublicReads } from "../board/refresh";
import { createWorldSwitch } from "../board/world";
import type { LotViewTarget } from "../catalog/detail/deep-link";
import { PublicSessionView, type PublicSessionView as PublicSessionViewType } from "../index";
import {
  InitialPublicLocationContext,
  type LiveConnectionState,
  PublicLocationRevisionContext,
  publicAuctionHomeQuery,
  readPublicQuery,
  setPublicQuery,
  usePublicQueryState,
  worldScopeHref,
  worldScopeNavigationWorld,
} from "../query";
import {
  publicConnectionClass,
  publicConnectionMessage,
  publicConnectionStateName,
} from "../realtime";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  SelectedWorldContext,
  WorldSwitchContext,
} from "./contexts";
import { PublicShellProviders } from "./providers";
import { createPublicShellSession, routeFocusToken, selectedWorldFromQuery } from "./session";
import { createPublicTickerLabels } from "./ticker";
import { PublicShellView } from "./view";

/** 权威数据轮询的基础间隔（页面可见时间）。 */
const AUTHORITATIVE_POLL_MILLIS = 60_000;
/** 实时连接正常时连续跳过的轮数；4 轮跳过加 1 轮刷新，即约每 5 分钟兜底一次。 */
const LIVE_POLL_SKIPPED_ROUNDS = 4;

export {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  SelectedWorldContext,
  WorldSwitchContext,
};

export function PublicShell(): JSX.Element {
  // 查询 hook 与页面共享同一个位置版本；提供方必须在外壳自身消费之前建立，不能等
  // 到渲染阶段的 provider 树。
  const locationGeneration = createCell(0);
  return (
    <PublicLocationRevisionContext value={{ revision: locationGeneration }}>
      <PublicShellContent locationGeneration={locationGeneration} />
    </PublicLocationRevisionContext>
  );
}

function PublicShellContent(props: { readonly locationGeneration: Cell<number> }): JSX.Element {
  useDialogFocusTrap(true);
  // 秒级共享时钟在外壳根提供；看板与 lot 详情的倒计时通过 use_second_tick 订阅同一节拍。
  const secondTicker = useSecondTicker();
  const router = useRouter();
  const currentRoute = () => router.current();
  const auctionPageOwnsInitialBoot = createValueMemo(() => {
    const route = currentRoute();
    return (
      route.kind === "matched" && (route.name === "AuctionIndex" || route.name === "AuctionPage")
    );
  });
  const routeToken = createValueMemo(() => routeFocusToken(currentRoute()));
  useRouteFocus(routeToken, "public-content");
  // world 同步覆盖 Router Link push、同路径 query 导航和浏览器前进/后退。
  const worldQueryState = usePublicQueryState();
  const worldQuerySignal = worldQueryState.state;
  const initialLocation = useContext(InitialPublicLocationContext);
  const selectedWorld = createCell(selectedWorldFromQuery(readPublicQuery(initialLocation).world));
  const pageVisibility = createPageVisibility();
  // 世界目录同步、单世界页纠偏与切换动作随外壳常驻；页面只渲染按钮组。
  const worldSwitch = createWorldSwitch({
    selectedWorld,
    shellQueryState: worldQuerySignal,
    pageVisibility,
  });
  const navigationWorld = createValueMemo(() => {
    const route = currentRoute();
    return worldScopeNavigationWorld(
      worldQuerySignal(),
      route.kind === "matched" ? route.name : null,
    );
  });
  const scopedHref = (path: string): string => worldScopeHref(path, navigationWorld());
  // 看板首页冷启动时与当前场次读取并行预取上次场次的 lots；只在首次渲染执行一次。
  if (auctionPageOwnsInitialBoot()) {
    prefetchBoardLots(selectedWorld.peek(), readPublicQuery(initialLocation));
  }
  const locationGeneration = props.locationGeneration;
  const navigationOpen = createCell(false);
  const sidebarCollapsed = createCell(loadSidebarCollapsed());
  const refreshGeneration = createCell(0);
  const manualRefreshBusy = createCell(false);
  const manualRefreshError = createCell(false);
  const bidRefreshRevision = createCell(0);
  const bidRefresh: PublicBidRefresh = {
    revision: bidRefreshRevision,
    notify: () =>
      bidRefreshRevision.update((value) => Math.min(value + 1, Number.MAX_SAFE_INTEGER)),
  };
  const readinessGeneration = createCell(0);
  // 游标归整个 Public 外壳；订阅因目标或用户身份变化重启时仍保留已确认位置。
  const auctionAfter = createCell<readonly [string, number] | null>(null);
  const stableRealtimeTarget = createCell<PublicRealtimeTarget>({ kind: "checking" });
  const pageRealtimeTarget = createCell<
    readonly [PublicRealtimePageKey, PublicRealtimeTarget] | null
  >(null);
  const realtimeState = createCell<LiveConnectionState>("connecting");
  const tasks = createTaskScope();
  // readiness 只在启动门禁与页面恢复可见时读取一次；持续连接状态由 realtime WS 与各 API
  // 的失败反馈承担，不按固定周期轮询 `/health/ready`。
  const readiness = createTaskResource(
    () => readinessGeneration(),
    async (_generation, context) => fetchPublicReadiness(context.signal),
  );
  const { session, sessionEndedElsewhere, commandOwner, checkSession } =
    createPublicShellSession(refreshGeneration);
  const currentAuction = createTaskResource(
    () => [refreshGeneration(), selectedWorld()] as const,
    async ([, world], context) => {
      const result = await fetchCurrentAuction(world, context.signal);
      if (context.alive() && result.ok && result.value !== null) {
        rememberCurrentAuction(world, result.value.id);
      }
      return [world, result] as const;
    },
  );
  // 页面可见性在轮询与实时重连退避之间共享；隐藏时全部挂起（不重置周期），
  // 恢复可见时立即补偿一次刷新并恢复各自节奏。
  tasks.spawn(async (context) => {
    let skippedWhileLive = 0;
    while (context.alive()) {
      await sleepWhileVisible(pageVisibility, AUTHORITATIVE_POLL_MILLIS, context.signal);
      if (!context.alive()) {
        return;
      }
      // 实时连接正常时，数据变化由事件推送、断线由游标回放补齐，轮询只作兜底。
      if (realtimeState.peek() === "live" && skippedWhileLive < LIVE_POLL_SKIPPED_ROUNDS) {
        skippedWhileLive += 1;
        continue;
      }
      skippedWhileLive = 0;
      // 权威轮询与客户端短缓存对齐：先失效再刷新，避免轮询命中旧 TTL。
      refreshPublicReads(refreshGeneration);
    }
  });
  onSettled(() => {
    // 恢复可见时立即补偿一次 readiness 与权威 current 重读，快速回到同步。隐藏期间读到
    // 的条目可能仍在 TTL 内：补读与轮询一样先失效公开读缓存，否则只会原样返回旧快照。
    pageVisibility.install(() => {
      worldSwitch.revalidateIfNeeded();
      readinessGeneration.update((value) => Math.min(value + 1, Number.MAX_SAFE_INTEGER));
      refreshPublicReads(refreshGeneration);
      checkSession("refresh");
    });
    return () => pageVisibility.cleanup();
  });
  const bootPrerequisitesReady = () =>
    readiness() !== undefined && session() !== undefined && currentAuction() !== undefined;
  // 默认拍卖页还要等待首屏 lots；其他路由在三项外壳请求都有结果后即可接管静态骨架。
  useAppBootReady(() => bootPrerequisitesReady() && !auctionPageOwnsInitialBoot());
  const displayedAuction = createCell<CurrentAuctionItemResponse | null>(null);
  const lotViewRequest = createCell<LotViewTarget | null>(null);
  const contentLoading = createCell<ReadonlySet<number>>(new Set());
  usePopstateRevision(locationGeneration);
  const readinessState = createValueMemo((): PublicReadinessState => {
    const result = readiness();
    if (result === undefined) {
      return "checking";
    }
    return result.ok && result.value ? "ready" : "degraded";
  });
  const sessionView = createValueMemo(
    (): PublicSessionViewType => PublicSessionView.fromSnapshot(session()),
  );
  const sessionPending = () => sessionView().pending;
  const authenticated = () => sessionView().authenticated;
  const realtimeUserId = (): string | null => {
    const snapshot = session();
    if (
      snapshot === undefined ||
      !snapshot.ok ||
      snapshot.value === null ||
      !snapshot.value.authenticated
    ) {
      return null;
    }
    return snapshot.value.user_id;
  };
  const currentPublicQuery = createValueMemo(() => {
    router.currentLocationString();
    locationGeneration();
    worldQuerySignal();
    return readPublicQuery(initialLocation);
  });
  const auctionHomeQuery = createValueMemo(() => publicAuctionHomeQuery(navigationWorld()));
  const auctionHomeHref = createValueMemo(() => scopedHref("/"));
  const auctionHomeNeedsQueryReset = () => {
    const route = currentRoute();
    return (
      route.kind === "matched" &&
      route.name === "AuctionIndex" &&
      !structuralEquals(currentPublicQuery(), auctionHomeQuery())
    );
  };
  const requestedPageTarget = createValueMemo((): PublicRealtimePageKey | null => {
    const route = currentRoute();
    if (
      route.kind !== "matched" ||
      (route.name !== "AuctionIndex" && route.name !== "AuctionPage")
    ) {
      return null;
    }
    const auctionId = currentPublicQuery().auctionId;
    return auctionId === null ? null : { kind: "auction", id: auctionId };
  });
  const realtimeTargetCandidate = createValueMemo((): PublicRealtimeTarget => {
    const snapshot = session();
    if (snapshot === undefined) {
      return { kind: "checking" };
    }
    if (!snapshot.ok) {
      return { kind: "session_error" };
    }
    const requested = requestedPageTarget();
    if (requested !== null) {
      const loaded = pageRealtimeTarget();
      if (loaded !== null && loaded[0].kind === requested.kind && loaded[0].id === requested.id) {
        const target = loaded[1];
        return target.kind === "auction" && !authenticated() ? { kind: "anonymous" } : target;
      }
      return { kind: "checking" };
    }
    const loadedCurrent = currentAuction();
    if (loadedCurrent !== undefined && loadedCurrent[0] === selectedWorld()) {
      const [loadedWorld, result] = loadedCurrent;
      if (loadedWorld !== selectedWorld()) {
        return { kind: "checking" };
      }
      if (!result.ok) {
        return { kind: "current_error", world: selectedWorld() };
      }
      const auction = result.value;
      if (auction === null) {
        return { kind: "inactive", world: selectedWorld(), preview: false };
      }
      if ((auction.status === "live" || auction.status === "final") && authenticated()) {
        return {
          kind: "auction",
          auctionId: auction.id,
          world: auction.world,
          previewRevision: auction.preview_revision,
        };
      }
      if (auction.status === "live" || auction.status === "final") {
        return { kind: "anonymous" };
      }
      return { kind: "inactive", world: selectedWorld(), preview: auction.status === "preview" };
    }
    return { kind: "checking" };
  });
  // 同一 world 的权威 current 快照重读期间保留既有目标；首屏、登出和 world 切换仍失败关闭。
  const realtimeTarget = createValueMemo((): PublicRealtimeTarget => {
    const candidate = realtimeTargetCandidate();
    if (candidate.kind === "checking" && authenticated() && requestedPageTarget() === null) {
      const remembered = stableRealtimeTarget();
      if (
        (remembered.kind === "auction" || remembered.kind === "inactive") &&
        remembered.world === selectedWorld()
      ) {
        return remembered;
      }
    }
    return candidate;
  });
  createEffect(realtimeTarget, (remembered) => {
    if (!structuralEquals(stableRealtimeTarget.peek(), remembered)) {
      stableRealtimeTarget.set(remembered);
    }
  });
  const connectionState = createValueMemo(
    (): PublicConnectionState =>
      publicConnectionState(
        readinessState(),
        sessionPending(),
        authenticated(),
        realtimeTarget(),
        realtimeState(),
      ),
  );
  const connectionClass = () => publicConnectionClass(connectionState());
  const connectionStateName = () => publicConnectionStateName(connectionState());
  const connectionMessage = () => publicConnectionMessage(connectionState());
  const connectionTitle = () => connectionMessage();
  const goHome = (): void => {
    let written = true;
    if (auctionHomeNeedsQueryReset()) {
      // 同路由的首页入口仍清除历史场次与筛选；两端导航共享同一写回动作。
      written = setPublicQuery(auctionHomeQuery(), false);
    }
    navigationOpen.set(false);
    if (written) {
      // 写入失败时地址没有变化，不能唤醒位置订阅者重读。
      locationGeneration.set(Math.min(locationGeneration() + 1, Number.MAX_SAFE_INTEGER));
    }
  };
  const manualRefresh = (): void => {
    if (manualRefreshBusy.peek()) {
      return;
    }
    manualRefreshError.set(false);
    manualRefreshBusy.set(true);
    tasks.spawn(async (context) => {
      await sleep(80, context.signal);
      if (!context.alive()) {
        return;
      }
      let reloaded = false;
      if (typeof window !== "undefined") {
        try {
          window.location.reload();
          reloaded = true;
        } catch {
          reloaded = false;
        }
      }
      if (!reloaded) {
        manualRefreshBusy.set(false);
        manualRefreshError.set(true);
        // 重新加载被拒时补读权威数据：同样先失效缓存，才是真正的重读。
        refreshPublicReads(refreshGeneration);
      }
    });
  };
  // ticker 承担页面标题：拍卖页按 auction_id 展示某一场次时显示正文同一场次，其余页面
  // 显示所选世界的当前场次。
  const tickerAuctionDisplay = createValueMemo((): CurrentAuctionItemResponse | null => {
    const snapshot = currentAuction();
    const current =
      snapshot !== undefined && snapshot[0] === selectedWorld() && snapshot[1].ok
        ? snapshot[1].value
        : null;
    return tickerAuction(requestedPageTarget(), displayedAuction(), current) ?? null;
  });
  const tickerLabels = createPublicTickerLabels(tickerAuctionDisplay, selectedWorld);

  return (
    <PublicShellProviders
      secondTicker={secondTicker}
      worldSwitch={worldSwitch}
      selectedWorld={selectedWorld}
      refreshGeneration={refreshGeneration}
      bidRefresh={bidRefresh}
      pageRealtimeTarget={pageRealtimeTarget}
      displayedAuction={displayedAuction}
      lotViewRequest={lotViewRequest}
      session={session}
      commandOwner={commandOwner}
      contentLoading={contentLoading}
      readiness={readiness}
      currentAuction={currentAuction}
      pageVisibility={pageVisibility}
    >
      <PublicShellView
        navigationOpen={navigationOpen}
        sidebarCollapsed={sidebarCollapsed}
        manualRefreshBusy={manualRefreshBusy}
        manualRefreshError={manualRefreshError}
        sessionEndedElsewhere={sessionEndedElsewhere}
        contentLoading={contentLoading}
        refreshGeneration={refreshGeneration}
        auctionAfter={auctionAfter}
        realtimeState={realtimeState}
        currentRoute={currentRoute}
        auctionHomeHref={auctionHomeHref}
        scopedHref={scopedHref}
        auctionHomeNeedsQueryReset={auctionHomeNeedsQueryReset}
        connectionClass={connectionClass}
        connectionStateName={connectionStateName}
        connectionTitle={connectionTitle}
        connectionMessage={connectionMessage}
        tickerTitle={tickerLabels.title}
        tickerStatus={tickerLabels.status}
        tickerWorld={tickerLabels.world}
        tickerCountdown={tickerLabels.countdown}
        realtimeUserId={realtimeUserId}
        realtimeTarget={realtimeTarget}
        onToggleNavigation={() => navigationOpen.set(!navigationOpen())}
        onCloseNavigation={() => navigationOpen.set(false)}
        onHome={goHome}
        onManualRefresh={manualRefresh}
        onToggleSidebar={() => {
          const next = !sidebarCollapsed();
          sidebarCollapsed.set(next);
          saveSidebarCollapsed(next);
        }}
      />
    </PublicShellProviders>
  );
}
