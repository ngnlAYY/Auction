//! Public 列表的统一分页选项、译文和查询状态更新；不改变 Admin 分页策略。

import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { type QueryWriteMode, readQueryPairs } from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { type PublicBoardQuery, readPublicQuery } from "../query";
import { currentBrowserSearch, publicQueryPage } from "../query/location";
import { MAX_PUBLIC_PAGE_OFFSET, publicOffsetLastPage } from "./bounds";
import { updatePublicQueryPairs, writePublicQueryPatch } from "./location";
import { savePublicPageSize } from "./preferences";

export { parsePublicPageSize, storedPublicPageSize } from "./preferences";

export const DEFAULT_PAGE_SIZE = 20;
export const PUBLIC_PAGE_SIZES = [10, 20, 50, 100] as const;

// 预设与默认值都必须是后端接受的页长，否则选择预设就会得到 400。
if (DEFAULT_PAGE_SIZE < 1 || DEFAULT_PAGE_SIZE > MAX_PUBLIC_PAGE_SIZE) {
  throw new Error("Public 默认页长超出契约范围");
}
if (PUBLIC_PAGE_SIZES[3] > MAX_PUBLIC_PAGE_SIZE) {
  throw new Error("Public 分页预设超出契约上限");
}

/** 每页数量只接受 1 到契约上限；超出的页长后端一律拒绝，不能写进 URL、偏好或请求。 */
export function acceptedPublicPageSize(size: number): number | null {
  return Number.isInteger(size) && size >= 1 && size <= MAX_PUBLIC_PAGE_SIZE ? size : null;
}

/** 非法或超过上限的数量回退默认值，不截断成上限。 */
export function normalizePublicPageSize(size: number): number {
  return acceptedPublicPageSize(size) ?? DEFAULT_PAGE_SIZE;
}

/** 切换数量与回到首页作为一次 URL 更新，保留世界、场次和筛选条件。 */
export function updatePublicPageSize(state: Cell<PublicBoardQuery>, size: number): boolean {
  return updatePublicQueryPairs(
    state,
    [...pageKeys(state), "page_size"],
    [["page_size", String(normalizePublicPageSize(size))]],
    "push",
  );
}

/** 翻页只替换页码，保留地址里的其他筛选原文。 */
export function updatePublicPage(
  state: Cell<PublicBoardQuery>,
  page: number,
  mode: QueryWriteMode,
): boolean {
  return updatePublicQueryPairs(
    state,
    pageKeys(state),
    page > 1 ? [["page", String(page)]] : [],
    mode,
  );
}

function pageKeys(state: Cell<PublicBoardQuery>): readonly string[] {
  return publicQueryPage(state) === "BidLog" ? ["page", "bidlog_page"] : ["page"];
}

/** 资源重读期间不能把旧页总数用于新分页状态的纠偏；可选字段沿用旧 DTO 兼容语义。 */
export function publicPageMatches(
  page: number | null | undefined,
  pageSize: number | null | undefined,
  requestedPage: number,
  requestedSize: number,
): boolean {
  return (
    (page == null || page === requestedPage) && (pageSize == null || pageSize === requestedSize)
  );
}

export interface PublicPagerProps {
  readonly page: number;
  readonly pageSize: number;
  readonly total: number | null;
  readonly hasMore?: boolean;
  readonly movement?: boolean;
  readonly maxOffset?: number | null;
  readonly hideWhenSingle?: boolean;
  readonly showSizePicker?: boolean;
  /** 页面级分页条：只给位于页面内容末尾的列表传 true，样式据此固定到页面底部。 */
  readonly pageLevel?: boolean;
  readonly jumpLabel?: string;
  readonly maxPageSize?: number | null;
  readonly label: string;
  /** 返回 false 时恢复跳页输入为已应用页码。 */
  readonly onPageChange: (page: number) => unknown;
  readonly onPageClamp?: (page: number) => unknown;
  /** 返回 `false` 表示地址栏写入失败；分页组件据此决定是否保存本地偏好。 */
  readonly onPageSizeChange?: (size: number) => unknown;
}

/** 页长偏好只能在地址栏写入成功后保存；未返回结果的旧回调需要核对写入值。 */
export function commitPublicPageSizeChoice(
  size: number,
  movement: boolean,
  writePageSize: ((size: number) => unknown) | undefined,
): boolean {
  const before = currentBrowserSearch();
  const result = writePageSize?.(size);
  const key = movement ? "movement_page_size" : "page_size";
  if (
    result !== true &&
    (result === false ||
      before === currentBrowserSearch() ||
      !readQueryPairs().some(([name, value]) => name === key && value === String(size)))
  ) {
    return false;
  }
  savePublicPageSize(size, movement);
  return true;
}

/** 注入 Public 五语言文案、1 到契约上限的数量与本地偏好，分页计算及键盘交互由共享组件负责。
 * `hideWhenSingle` 保留挂载但隐藏控件；`showSizePicker` 关闭数字页码与数量选择器时保留经典布局。
 */
export function PublicPager(props: PublicPagerProps): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.pageSize, 1));
  const maximum = createValueMemo(() =>
    Math.min(props.maxPageSize ?? MAX_PUBLIC_PAGE_SIZE, MAX_PUBLIC_PAGE_SIZE),
  );
  const maxOffset = createValueMemo(() =>
    props.maxOffset === undefined ? MAX_PUBLIC_PAGE_OFFSET : props.maxOffset,
  );
  const maxPage = createValueMemo(() => {
    const offset = maxOffset();
    return offset === null ? null : publicOffsetLastPage(pageSize(), offset);
  });
  const movement = createValueMemo(() => props.movement ?? false);
  const total = () => props.total;
  const singlePage = () => {
    const count = total();
    // 原页仍待纠偏时保留重试入口，即使新总数只剩一页。
    return props.page <= 1 && count !== null && count <= pageSize();
  };
  const pager = (
    <Pager
      page={props.page}
      pageSize={pageSize()}
      total={total()}
      hasMore={props.hasMore ?? false}
      maxPage={maxPage()}
      label={props.label}
      previousLabel={t("catalog-previous")}
      status={(value) => t("catalog-page", { page: value[0], pages: value[1] })}
      nextLabel={t("catalog-next")}
      jumpLabel={props.jumpLabel ?? t("pagination-jump")}
      jumpButtonLabel={t("catalog-page-go")}
      totalSummary={total() === null ? "" : t("pagination-total", { count: total() ?? 0 })}
      rangeSummary={(value) =>
        t("pagination-range", {
          start: String(value[0]),
          end: String(value[1]),
          total: String(value[2]),
        })
      }
      pageSizeOptions={props.showSizePicker === false ? [] : PUBLIC_PAGE_SIZES}
      pageSizeLabel={t("pagination-size")}
      pageSizeOptionLabel={(count) => t("pagination-size-option", { count: String(count) })}
      maxPageSize={maximum()}
      allowCustomPageSize={true}
      pageSizeCustomLabel={t("pagination-custom")}
      pageSizeApplyLabel={t("pagination-apply")}
      pageSizeInvalidLabel={t("pagination-size-limit", { maximum: String(maximum()) })}
      onPageChange={props.onPageChange}
      pageLevel={props.pageLevel === true}
      onPageClamp={props.onPageClamp ?? null}
      clampFailure={{ message: t("pagination-clamp-error"), retryLabel: t("data-retry") }}
      onPageSizeChange={(size) => {
        // 共享分页器已拒绝超出上限的数量；这里再守一次，不把它写进地址栏与偏好。
        if (acceptedPublicPageSize(size) === null || size > maximum()) {
          return false;
        }
        // 先冻结离开条目的数量（含 10），再更新 URL 和偏好；返回时不读到新偏好。
        // 只替换数量键，不能覆盖出价日志尚在防抖写回的筛选。
        const previous = readPublicQuery();
        const sizes = [
          ["page_size", String(previous.pageSize)],
          ["movement_page_size", String(previous.movementPageSize)],
        ] as const;
        const applied = readQueryPairs();
        const missing = sizes.filter(([key]) => !applied.some(([name]) => name === key));
        if (
          missing.length !== 0 &&
          !writePublicQueryPatch(
            missing.map(([key]) => key),
            missing,
            "replace",
          )
        ) {
          return false;
        }
        return commitPublicPageSizeChoice(size, movement(), props.onPageSizeChange);
      }}
    />
  );
  return (
    <Show when={props.hideWhenSingle} fallback={pager}>
      <div hidden={singlePage()}>{pager}</div>
    </Show>
  );
}
