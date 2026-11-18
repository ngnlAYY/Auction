// 目录行进入详情抽屉的入口：整行点击、行聚焦后的 Enter/Space 与操作栏查看按钮共用同一
// 打开动作；行内自带交互的控件（装备链接、出价、标记、可展开的截断文本）保持各自行为。

import { createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { useRouter } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import type { Accessor } from "solid-js";
import { createEffect, onCleanup, untrack, useContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { CatalogActionIcon } from "../actions";
import { CommandDialogContext } from "../command-scope";
import { isActivationKey } from "../presentation";
import type { CatalogDetailBidSlot, CatalogDetailInput } from ".";
import { CatalogDetailDialogContext, catalogDetailBidDefault } from ".";
import { LotViewRequestContext } from "./deep-link";

/** 行本身与行内自带交互的区域。事件目标向上最近的匹配项是行时，事件才属于“整行”；
 * 操作栏整体算控件区，图标之间的空隙点偏了也不会改成打开抽屉。 */
const ROW_OR_CONTROL_SELECTOR =
  "a[href], button, input, select, textarea, summary, [tabindex], .public-auction-catalog__action-controls, tr";

/** 一行的详情入口。`bid` 由该行操作栏写入，行只负责把它连同行快照交给抽屉。 */
export interface CatalogDetailEntry {
  /** 出价入口投影；没有操作栏的行（预览、已结算、已结束拍品）保持 `unavailable`。 */
  readonly bid: CatalogDetailBidSlot;
  /** 以当前行快照打开抽屉。 */
  open(): void;
  /** 整行点击：落在行内控件上的点击与结束文本拖选的点击都不打开抽屉。 */
  click(event: MouseEvent): void;
  /** 行自身聚焦时的 Enter/Space；行内控件冒泡上来的按键仍归控件自己。 */
  keydown(event: KeyboardEvent): void;
}

function createEntry(bid: CatalogDetailBidSlot, open: () => void): CatalogDetailEntry {
  return {
    bid,
    open,
    click(event) {
      if (!targetsRow(event.target) || selectingText()) {
        return;
      }
      open();
    },
    keydown(event) {
      if (!isActivationKey(event.key)) {
        return;
      }
      if (!targetsRow(event.target)) {
        return;
      }
      // 行聚焦时 Space 的默认行为是滚动页面。
      event.preventDefault();
      // 长按产生的重复按键不再次打开：抽屉被关闭、焦点回到本行后，同一次长按不会把它重新弹出。
      if (event.repeat) {
        return;
      }
      open();
    },
  };
}

/** 建立一行的详情入口，并让已打开的抽屉跟随同一拍品的行投影。
 * `watchable` 与行内是否渲染标记按钮保持同一条件。
 *
 * 这两个参数用访问器传入：行组件不会在目录重读时重建，抽屉必须看到最新的行快照与出价投影。
 */
export function useCatalogDetailEntry(
  row: Accessor<CatalogRow>,
  watchable: Accessor<boolean>,
): CatalogDetailEntry {
  const detail = useContext(CatalogDetailDialogContext);
  const commandDialog = useContext(CommandDialogContext);
  const bid = createCell(catalogDetailBidDefault());
  const currentLotId = createValueMemo(() => row().lotId);
  // 行是抽屉快照的唯一写入方：打开中的抽屉随实时价格、领先者与出价资格更新，
  // 抽屉自身不订阅目录、不重算资格，因此不会停在点击瞬间的旧价格上。
  const dependency = createValueMemo(() => ({
    row: row(),
    bid: bid(),
    watchable: watchable(),
  }));
  createEffect(
    () => dependency(),
    (current) => {
      const openInput = detail.peek();
      if (openInput === null || openInput.row.lotId !== current.row.lotId) {
        return;
      }
      const next: CatalogDetailInput = {
        row: structuredClone(current.row),
        bid: structuredClone(current.bid),
        watchable: current.watchable,
      };
      if (!structuralEquals(next, openInput)) {
        detail.set(next);
      }
    },
  );
  const droppedLot = row().lotId;
  onCleanup(() => {
    // 分页、筛选或目录读取失败会卸载行：抽屉保留最后一次展示快照，但出价入口失败关闭，
    // 不留下一份无人更新的资格。身份宿主先于行销毁时信号已不可写，直接跳过。
    const current = detail.peek();
    if (current !== null && current.row.lotId === droppedLot) {
      detail.set({ ...current, bid: catalogDetailBidDefault() });
    }
  });
  const open = () => {
    // 模态命令弹窗打开期间不在其下方再开抽屉。
    if (commandDialog != null && commandDialog.peek() !== null) {
      return;
    }
    detail.set({
      row: structuredClone(row()),
      bid: structuredClone(bid.peek()),
      watchable: watchable(),
    });
  };
  // 拍品直达链接：请求指向本行时由本行打开抽屉并清空请求。行是抽屉快照的唯一写入方，
  // 这样打开的抽屉与点击本行完全一样，出价入口照常跟随操作栏投影。
  const router = useRouter();
  const viewRequest = useContext(LotViewRequestContext);
  createEffect(
    () => ({ request: viewRequest(), lotId: currentLotId() }),
    ({ request, lotId }) => {
      if (
        request?.target.rowReady === true &&
        request.target.lotId === lotId &&
        viewRequest.peek() === request &&
        request.entryKey === untrack(router.currentEntryKey)
      ) {
        viewRequest.set(null);
        untrack(open);
      }
    },
  );
  return createEntry({ cell: bid }, open);
}

/** 操作栏的查看入口：任何阶段的目录行都有，与整行点击打开同一个抽屉。 */
export function CatalogViewButton(props: {
  readonly lotCode: string;
  readonly onOpen: () => void;
}): JSX.Element {
  return (
    <button
      class="public-auction-catalog__icon-action"
      type="button"
      aria-haspopup="dialog"
      // 可访问名称带拍品编号，逐行可区分；悬停提示保持简短动作名。
      aria-label={t("catalog-view-lot", { code: props.lotCode })}
      title={t("catalog-view")}
      onClick={() => props.onOpen()}
    >
      <CatalogActionIcon kind="eye" />
    </button>
  );
}

/** 事件目标是否属于行本身，而不是行内自带交互的控件。 */
function targetsRow(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) {
    return false;
  }
  const nearest = target.closest(ROW_OR_CONTROL_SELECTOR);
  return nearest !== null && nearest.tagName.toLowerCase() === "tr";
}

/** 拖选行内文本（例如复制装备名）同样以一次 click 结束，此时选区非空。 */
function selectingText(): boolean {
  const selection = window.getSelection();
  return selection !== null && !selection.isCollapsed;
}
