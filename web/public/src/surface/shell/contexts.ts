//! 外壳向页面提供的世界切换、世界、就绪与可见性上下文。

import type { PageVisibility } from "@auction/web-shared/platform/visibility";

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createContext } from "solid-js";
import type { WorldSwitch } from "../board/world";
import type { CurrentAuctionResource, PublicReadinessResource } from "../index";

export const WorldSwitchContext = createContext<WorldSwitch>();
export const SelectedWorldContext = createContext<Cell<string>>();
export const PublicReadinessContext = createContext<PublicReadinessResource>();
export const CurrentAuctionContext = createContext<CurrentAuctionResource>();
export const PageVisibilityContext = createContext<PageVisibility>();
/** 只有外壳权威确认同账号重新登录时才推进。 */
export const PreferenceReloadRevisionContext = createContext<() => number>(() => 0);
