// “我的”页查看抽屉的出价接入：没有目录行，因此由这里补上行与操作栏的角色。
// 出价资格仍只由 `LiveCatalogActions` 判定（无头挂载，只写抽屉的出价投影），命令经
// `CatalogCommandScope` 的同一个弹窗提交，抽屉本身不重算资格、不创建 `command_id`。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import type { JSX } from "@solidjs/web";
import { createComponent, createEffect, Show, untrack, useContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { fetchHistoricalAuction } from "../../../api/queries/current";
import { PublicSessionResourceContext, PublicSessionView } from "../..";
import { PublicRefreshGenerationContext } from "../../board/refresh";
import { CatalogProxyScope, LiveCatalogActions } from "../actions";
import { CatalogCommandScope } from "../command-scope";
import type { DetailWorld } from ".";
import { CatalogDetailDialogContext } from ".";
import { CatalogDetailDrawer } from "./drawer";
import { useCatalogDetailEntry } from "./entry";

/** 带出价入口的只读查看抽屉。`row` 随详情读取实时更新；进行中的拍品先读取所在场次
 * （世界、是否开放出价、代理默认模式），读取失败时不提供入口（失败关闭）。 */
export function LookupBiddingDrawer(props: {
  readonly userId: string;
  readonly auctionId: string;
  readonly row: CatalogRow;
  readonly world: DetailWorld;
  readonly onClose: () => void;
}): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const live = createValueMemo(() => props.row.status === "live");
  // 只有进行中的拍品需要场次上下文；结束后的拍品不发读取，与目录里已结束的行一样没有出价入口。
  const auctionRead = createTaskResource(
    () => (live() ? { auctionId: props.auctionId, refresh: refreshGeneration() } : null),
    async (dependencies, context) => {
      if (dependencies === null) return null;
      const read = await fetchHistoricalAuction(dependencies.auctionId, context.signal);
      return context.alive() ? read : null;
    },
  );
  const auction = createValueMemo(() => {
    const read = auctionRead();
    return read?.ok === true ? read.value : null;
  });
  // 首次场次读取完成前不展示抽屉，避免出价入口在打开后才出现而整块重排。
  const ready = createValueMemo(() => !live() || auctionRead() !== undefined);
  return (
    <CatalogCommandScope
      userId={props.userId}
      auction={auction()}
      auctionInvalid={false}
      followsPageQuery={false}
    >
      <CatalogProxyScope auctionId={props.auctionId}>
        <LookupBidProjection row={props.row} auction={auction()} ready={ready()} />
        <LookupDrawer auctionId={props.auctionId} world={props.world} onClose={props.onClose} />
      </CatalogProxyScope>
    </CatalogCommandScope>
  );
}

/** 写入抽屉快照与出价投影：与目录行相同，行快照由 `useCatalogDetailEntry` 维护，出价资格由
 * 无头的 `LiveCatalogActions` 写入。 */
export function LookupBidProjection(props: {
  readonly row: CatalogRow;
  readonly auction: CurrentAuctionItemResponse | null;
  readonly ready: boolean;
}): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const sessionView = createValueMemo(() => PublicSessionView.fromSnapshot(session()));
  const entry = useCatalogDetailEntry(
    () => props.row,
    () => false,
  );
  let opened = false;
  createEffect(
    () => props.ready,
    (ready) => {
      if (ready && !opened) {
        opened = true;
        untrack(() => entry.open());
      }
    },
  );
  // 与目录一致：只有竞价展示期内仍在竞价的拍品才挂出价资格判定。
  const biddable = createValueMemo(() =>
    props.row.status === "live" && props.auction?.presentation_mode === "live"
      ? props.auction
      : null,
  );
  const leading = createValueMemo(() => {
    const viewer = sessionView().viewerPlayerId;
    return viewer !== null && props.row.leaderName !== null && viewer === props.row.leaderName;
  });
  // 以 createComponent 书写而非 JSX：测试运行时把 JSX 求值为 null，出价投影需要在测试里真实挂载。
  // 场次字段在不可出价时都取失败关闭的值；`Show` 保证只有可出价时才挂载。
  return createComponent(Show<boolean>, {
    get when() {
      return biddable() !== null;
    },
    get children() {
      return createComponent(LiveCatalogActions, {
        headless: true,
        get lotId() {
          return props.row.lotId;
        },
        get lotCode() {
          return props.row.code;
        },
        get lotName() {
          return props.row.name;
        },
        get canBid() {
          return biddable()?.bidding_open === true;
        },
        get currentPrice() {
          return props.row.currentPrice;
        },
        get nextBid() {
          return props.row.nextBid;
        },
        get lotStatus() {
          return props.row.status;
        },
        get allowSlow() {
          return biddable()?.status === "live";
        },
        get proxyDefaultMode() {
          return biddable()?.proxy_default_mode ?? null;
        },
        get sessionReady() {
          return sessionView().ready;
        },
        get authenticated() {
          return sessionView().authenticated;
        },
        get leading() {
          return leading();
        },
        detailBid: entry.bid,
      });
    },
  });
}

/** 抽屉只渲染作用域里的展示快照，由 `LookupBidProjection` 打开并持续更新。 */
function LookupDrawer(props: {
  readonly auctionId: string;
  readonly world: DetailWorld;
  readonly onClose: () => void;
}): JSX.Element {
  const detail = useContext(CatalogDetailDialogContext);
  return (
    <Show when={detail()}>
      {(input) => (
        <CatalogDetailDrawer
          auctionId={props.auctionId}
          input={input()}
          world={props.world}
          onClose={props.onClose}
        />
      )}
    </Show>
  );
}
