// Admin 场次页的草稿宿主与两个确认对话框的装配。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { createContext, Show, useContext } from "solid-js";

import type { AdminAuction } from "../../../api/auctions";
import type { AdminAuctionLifecycleAction } from "../../../api/lifecycle";
import type { DraftRead } from "../editor";
import {
  AdminAuctionActionDialog,
  type LifecycleDraft,
  type LifecycleTarget,
  lifecycleDraftAuctionId,
  lifecycleDraftNew,
} from "./dialog";
import type { AdminAuctionNotice } from "./model";
import {
  AdminAuctionScheduleDialog,
  type ScheduleDraft,
  scheduleDraftFromAuction,
} from "./schedule";

/** 页级草稿宿主。在读取失败、换页或对应行卸载时保留草稿，当前页只打开一个编辑框。 */
export interface AuctionDrafts {
  readonly lifecycle: Cell<LifecycleDraft | null>;
  readonly schedule: Cell<ScheduleDraft | null>;
  auctionId(): string | null;
  openSchedule(auction: AdminAuction): boolean;
  openLifecycle(target: LifecycleTarget, action: AdminAuctionLifecycleAction): boolean;
}

/** 场次页及其子组件读取同一份草稿；缺少提供方是装配错误。 */
export const AuctionDraftsContext = createContext<AuctionDrafts | null>(null);

export function useAuctionDrafts(): AuctionDrafts {
  const lifecycle = createCell<LifecycleDraft | null>(null);
  const schedule = createCell<ScheduleDraft | null>(null);
  const isOpen = (): boolean => schedule.peek() !== null || lifecycle.peek() !== null;
  return {
    lifecycle,
    schedule,
    auctionId: () => {
      const scheduleDraft = schedule();
      if (scheduleDraft !== null) {
        return scheduleDraft.auctionId;
      }
      const lifecycleDraft = lifecycle();
      return lifecycleDraft === null ? null : lifecycleDraftAuctionId(lifecycleDraft);
    },
    openSchedule: (auction) => {
      if (isOpen()) {
        return false;
      }
      const draft = scheduleDraftFromAuction(auction);
      if (draft === null) {
        return false;
      }
      schedule.set(draft);
      return true;
    },
    openLifecycle: (target, action) => {
      if (isOpen()) {
        return false;
      }
      lifecycle.set(lifecycleDraftNew(target, action));
      return true;
    },
  };
}

/** 读取场次页提供的草稿宿主。 */
export function requireAuctionDrafts(): AuctionDrafts {
  const drafts = useContext(AuctionDraftsContext);
  if (drafts === null) {
    throw new Error("场次草稿宿主缺失");
  }
  return drafts;
}

export interface AdminAuctionDraftDialogsProps {
  readonly drafts: AuctionDrafts;
  readonly latestVersion: number | null;
  readonly read: DraftRead;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly reloadGeneration: Cell<bigint>;
}

export function AdminAuctionDraftDialogs(props: AdminAuctionDraftDialogsProps): JSX.Element {
  return (
    <>
      <Show when={props.drafts.schedule()}>
        <AdminAuctionScheduleDialog
          draft={props.drafts.schedule}
          latestVersion={props.latestVersion}
          read={props.read}
          notice={props.notice}
          reloadGeneration={props.reloadGeneration}
        />
      </Show>
      <Show when={props.drafts.lifecycle()}>
        <AdminAuctionActionDialog
          latestVersion={props.latestVersion}
          notice={props.notice}
          pending={props.drafts.lifecycle}
          reloadGeneration={props.reloadGeneration}
          read={props.read}
        />
      </Show>
    </>
  );
}
