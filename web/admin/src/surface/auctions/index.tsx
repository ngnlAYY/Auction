// Admin 拍卖列表页面。
// 当前页面展示服务端的拍卖生命周期快照，并通过独立的版本化 API 提供写命令入口；未配置
// 数据库的部署由服务端返回能力不可用（`feature_not_supported`）。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, type Result } from "@auction/web-shared/result";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { type AdminAuctionPage, fetchAdminAuctions } from "../../api/auctions";
import { useAdminSessionHost } from "../auth/shell";
import { useAdminPagination } from "../pagination";
import { PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { adminApiErrorMessage } from "../presentation";
import { type AuctionsQuery, useAuctionsQueryState } from "../query/auctions";
import { AdminAuctionDraftDialogs, AuctionDraftsContext, useAuctionDrafts } from "./actions";
import type { AdminAuctionNotice } from "./actions/model";
import { draftReadIsReady, useDraftPage } from "./editor";
import { AdminAuctionTable } from "./table";

/** 加载并展示 Admin 拍卖列表。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  // 世界筛选与分页跟地址栏双向同步：刷新、分享链接和浏览器前进/后退都能恢复视图。
  const query = useAuctionsQueryState();
  const queryState = query.state;
  const reloadGeneration = createCommandReload(0n);
  const pending = usePendingCommands(reloadGeneration, (entity) => {
    return (
      entity === "create-auction" || entity.startsWith("auction:") || entity.startsWith("intake:")
    );
  });
  const pagination = useAdminPagination("auctions", ["page_size"], 100, 100_000);
  const drafts = useAuctionDrafts();
  const notice = createCell<AdminAuctionNotice | null>(null);
  const editedAuction = createValueMemo(() => drafts.auctionId());
  const editing = createValueMemo(() => editedAuction() !== null);
  const [read, auctions] = useDraftPage(
    pagination,
    reloadGeneration,
    editing,
    () => {
      // 只把服务端筛选和页码作为请求身份；本地排序变化不能重发列表请求。
      const current = queryState();
      return [current.world, current.page] as const;
    },
    (request, size, signal) => {
      const page = request[1];
      if (page > BigInt(Number.MAX_SAFE_INTEGER)) {
        return Promise.resolve(err("admin_pagination_bounds"));
      }
      return fetchAdminAuctions(host, request[0], Number(page), size, signal);
    },
  );
  const latestVersion = createValueMemo((): number | null => {
    const auctionId = editedAuction();
    if (auctionId === null) {
      return null;
    }
    const current = auctions();
    if (current === null || !current.ok) {
      return null;
    }
    const found = current.value.auctions.find((auction) => auction.id === auctionId);
    return found === undefined ? null : found.state_version;
  });
  const state = createValueMemo((): Result<AdminAuctionPage, string> | null => {
    const current = auctions();
    if (current === null) {
      return null;
    }
    return current.ok ? current : err(adminApiErrorMessage(current.error));
  });
  const pendingInDialog = createValueMemo(() => {
    const auctionId = editedAuction();
    if (auctionId === null) {
      return false;
    }
    const entity = `auction:${auctionId}`;
    return pending.contains(entity) && !pending.isBusy(entity);
  });

  return (
    <AuctionDraftsContext value={drafts}>
      <PendingCommandsContext value={pending}>
        <Show when={!pendingInDialog()}>
          <PendingCommandPanel owner={pending} />
        </Show>
        <AdminAuctionTable
          state={state()}
          pagination={pagination}
          editsReady={draftReadIsReady(read())}
          world={queryState().world}
          sort={queryState().sort}
          direction={queryState().direction}
          onQueryChange={(next: AuctionsQuery) => query.push(next)}
          onRetry={() => advanceCommandReload(reloadGeneration)}
          onSortChange={([sort, direction]) => {
            const next = { ...queryState(), sort, direction };
            query.push(next);
          }}
          notice={notice}
        />
        <Show when={editing()}>
          <AdminAuctionDraftDialogs
            drafts={drafts}
            latestVersion={latestVersion()}
            read={read()}
            notice={notice}
            reloadGeneration={reloadGeneration}
          />
        </Show>
      </PendingCommandsContext>
    </AuctionDraftsContext>
  );
}
