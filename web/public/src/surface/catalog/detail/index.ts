// Public 目录的页内拍品详情：行入口、右侧抽屉、竞价历史与价格历史，以及目录之外的只读入口
// 和站外的拍品直达链接。这里只保存展示快照与出价入口投影；资格判定归操作栏，命令负载、
// `command_id` 与未知结果仍由命令宿主唯一持有。关闭、遮罩点击与 Escape 只清理展示状态，
// 不写 URL 或真实状态。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import type { CommandDialogInput } from "../command-scope";

/** 抽屉底部出价入口的投影。由行内操作栏按同一套资格判定写入，抽屉只照此渲染：
 * 目录行与抽屉因此不会对“能否出价”给出两种答案。 */
export type CatalogDetailBid =
  /** 非竞价期、会话未确认、达到金额上限或行已卸载：不渲染入口（失败关闭）。 */
  | { readonly kind: "unavailable" }
  /** 匿名访客且拍品可出价：渲染带安全 `return_to` 的登录链接。 */
  | { readonly kind: "login" }
  /** 已登录：与行内出价按钮相同的命令弹窗输入与禁用判定。 */
  | {
      readonly kind: "command";
      readonly input: CommandDialogInput;
      readonly disabled: boolean;
    };

/** 默认投影：非竞价期、会话未确认、达到金额上限或行已卸载时不渲染入口（失败关闭）。 */
export function catalogDetailBidDefault(): CatalogDetailBid {
  return { kind: "unavailable" };
}

/** 操作栏写入出价入口投影的句柄。对目录模块之外不透明：其他页面的只读抽屉不接操作栏，
 * 也就看不到命令弹窗输入。 */
export interface CatalogDetailBidSlot {
  readonly cell: Cell<CatalogDetailBid>;
}

/** 抽屉展示的目录行快照与当前出价入口；同一时刻只展示一个拍品。 */
export interface CatalogDetailInput {
  readonly row: CatalogRow;
  readonly bid: CatalogDetailBid;
  /** 该行是否提供标记入口；抽屉只在行内可标记时渲染同一个标记按钮。 */
  readonly watchable: boolean;
}

/** 抽屉内价格历史查询所用世界的来源。拍品响应不带世界，由打开抽屉的入口声明。 */
export type DetailWorld =
  | { readonly kind: "selected" }
  | { readonly kind: "known"; readonly world: string }
  | { readonly kind: "auction" };

/** 页面级详情抽屉信号。信号由身份宿主持有（身份切换即清空），目录行是唯一写入方，
 * 抽屉宿主只读取并负责关闭。 */
export const CatalogDetailDialogContext = createContext<Cell<CatalogDetailInput | null>>();
