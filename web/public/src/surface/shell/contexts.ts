//! 外壳向页面提供的世界、查询、就绪与可见性上下文。

import type { PageVisibility } from "@auction/web-shared/platform/visibility";

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createContext } from "solid-js";
import type { CurrentAuctionResource, PublicReadinessResource } from "../index";
import type { PublicBoardQuery } from "../query";

export const SelectedWorldContext = createContext<Cell<string>>();
export const PublicWorldQueryContext = createContext<Cell<PublicBoardQuery>>();
export const PublicReadinessContext = createContext<PublicReadinessResource>();
export const CurrentAuctionContext = createContext<CurrentAuctionResource>();
export const PageVisibilityContext = createContext<PageVisibility>();
