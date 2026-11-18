// 外壳保留同一账号在各场次读取的代理快照，供目录行恢复出价判定。

import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import type { Ok, Result } from "@auction/web-shared/result";
import { createContext } from "solid-js";
import type { OwnedProxy } from "../../api/account";

/** 一次授权读取的结果。 */
export type ProxyRead = Result<OwnedProxy | null, string>;

/** 快照的存活边界与已确认的账号归属一致。 */
export function createCatalogProxySnapshots() {
  let owner: string | null = null;
  const snapshots = createCell<ReadonlyMap<string, Ok<OwnedProxy | null>>>(new Map());
  return {
    replaceIdentity(userId: string | null) {
      if (owner !== userId) {
        owner = userId;
        snapshots.set(new Map());
      }
    },
    read(auctionId: string, lotId: string): Ok<OwnedProxy | null> | null {
      return snapshots().get(`${auctionId}:${lotId}`) ?? null;
    },
    remember(
      userId: string | null,
      auctionId: string,
      lotId: string,
      result: Ok<OwnedProxy | null>,
    ): void {
      // 离开目录或同账号复核保留已读授权；换号后旧结果不能写进新宿主。
      if (userId === null || userId !== owner) return;
      const key = `${auctionId}:${lotId}`;
      if (!structuralEquals(snapshots.peek().get(key), result)) {
        const next = new Map(snapshots.peek());
        next.set(key, result);
        snapshots.set(next);
      }
    },
  };
}

export const CatalogProxySnapshotsContext =
  createContext<ReturnType<typeof createCatalogProxySnapshots>>();
