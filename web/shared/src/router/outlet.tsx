// 路由出口：按当前路由名渲染页面，支持直接组件与按需加载的页面。
// 加载期间保留上一个页面，首次加载不渲染内容；被新导航取代的加载结果不会覆盖当前页面。

import type { JSX } from "@solidjs/web";
import { dynamic } from "@solidjs/web";
import type { Component } from "solid-js";
import { createEffect, Match, Show, Switch } from "solid-js";

import { createCell } from "../reactive/cell";
import type { TaskHandle } from "../reactive/tasks";
import { createTaskScope } from "../reactive/tasks";
import { useRouter } from "./context";
import type { RouteMatch, RouteParams, RouteQuery } from "./pattern";

/** 页面组件收到的路由参数。 */
export interface RoutePageProps {
  readonly params: RouteParams;
  readonly query: RouteQuery;
}

/** 可以直接渲染的页面组件。 */
export type RoutePageComponent = Component<RoutePageProps>;

/** 按需加载的页面函数。 */
export type RoutePageLoader = () => Promise<RoutePageComponent>;

/** 直接渲染的页面表项。 */
export interface RouteComponentPage {
  readonly kind: "component";
  readonly component: RoutePageComponent;
}

/** 按需加载的页面表项。 */
export interface RouteLazyPage {
  readonly kind: "lazy";
  readonly load: RoutePageLoader;
}

/** 路由名到页面表项的可辨识联合。 */
export type RoutePage = RouteComponentPage | RouteLazyPage;

/** 构造直接渲染的页面表项。 */
export function pageComponent(component: RoutePageComponent): RouteComponentPage {
  return { kind: "component", component };
}

/** 构造按需加载的页面表项。 */
export function lazyPage(load: RoutePageLoader): RouteLazyPage {
  return { kind: "lazy", load };
}

/** RouteOutlet 的属性。 */
export interface RouteOutletProps {
  readonly pages: Readonly<Record<string, RoutePage>>;
  readonly notFound: JSX.Element;
  /** 当前导航的按需加载失败时调用；应用据此显示入口文档已有的错误横幅。 */
  readonly onLoadError?: (error: unknown) => void;
}

interface ShownPage {
  readonly component: RoutePageComponent;
  readonly params: RouteParams;
  readonly query: RouteQuery;
}

interface PageCacheEntry {
  readonly promise: Promise<RoutePageComponent>;
  component: RoutePageComponent | null;
}

interface SelectedPage {
  readonly route: RouteMatch;
  readonly page: RoutePage | null;
}

// 动态 import 本身由浏览器按模块缓存；这里再按加载函数身份缓存结果，使返回同一路由时同步渲染。
const PAGE_CACHE = new WeakMap<RoutePageLoader, PageCacheEntry>();

function pageCacheEntry(loader: RoutePageLoader): PageCacheEntry {
  const existing = PAGE_CACHE.get(loader);
  if (existing !== undefined) {
    return existing;
  }
  const promise = Promise.resolve().then(() => loader());
  const entry: PageCacheEntry = { promise, component: null };
  PAGE_CACHE.set(loader, entry);
  // 失败不缓存，返回同一路由时允许重新加载；当前等待任务的拒绝由 RouteOutlet 显式处理。
  void promise.catch(() => {
    if (PAGE_CACHE.get(loader) === entry) {
      PAGE_CACHE.delete(loader);
    }
  });
  return entry;
}

function shownPage(component: RoutePageComponent, route: RouteMatch): ShownPage | null {
  if (route.kind !== "matched") {
    return null;
  }
  return { component, params: route.params, query: route.query };
}

function RoutePageHost(props: { readonly page: ShownPage }): JSX.Element {
  const Page = dynamic(() => props.page.component);
  return <Page params={props.page.params} query={props.page.query} />;
}

/** 当前路由的页面出口。 */
export function RouteOutlet(props: RouteOutletProps): JSX.Element {
  const router = useRouter();
  const tasks = createTaskScope();
  const shown = createCell<ShownPage | null>(null);
  let running: TaskHandle | undefined;

  createEffect(
    (): SelectedPage => {
      const route = router.current();
      if (route.kind !== "matched") {
        return { route, page: null };
      }
      return { route, page: props.pages[route.name] ?? null };
    },
    (selected) => {
      running?.cancel();
      running = undefined;
      if (selected.route.kind !== "matched" || selected.page === null) {
        shown.set(null);
        router.restoreScrollAfterCommit();
        return;
      }
      if (selected.page.kind === "component") {
        shown.set(shownPage(selected.page.component, selected.route));
        router.restoreScrollAfterCommit();
        return;
      }
      const loader = selected.page.load;
      const route = selected.route;
      const entry = pageCacheEntry(loader);
      if (entry.component !== null) {
        shown.set(shownPage(entry.component, route));
        router.restoreScrollAfterCommit();
        return;
      }
      // 目标页面的代码还没到：先卸载上一个地址的页面，而不是让它留到新页面就绪。
      // 旧页面仍然挂载时会把新地址当成自己的地址来读——纠偏类的 effect 会把自己的查询参数
      //（例如所选场次）写进新地址，数据读取也会按新地址重发。
      shown.set(null);
      running = tasks.spawn(async (context) => {
        try {
          const component = await entry.promise;
          if (!context.alive() || router.current() !== route) {
            return;
          }
          entry.component = component;
          shown.set({ component, params: route.params, query: route.query });
          router.restoreScrollAfterCommit();
        } catch (error) {
          if (!context.alive() || router.current() !== route) {
            return;
          }
          // 当前导航加载失败时不能继续显示上一个地址的页面；交给应用显示入口错误横幅。
          shown.set(null);
          props.onLoadError?.(error);
        }
      });
    },
  );

  const hasPage = (): boolean => {
    const route = router.current();
    return route.kind === "matched" && props.pages[route.name] !== undefined;
  };

  return (
    <Switch fallback={props.notFound}>
      <Match when={hasPage()}>
        <Show when={shown()}>{(page) => <RoutePageHost page={page()} />}</Show>
      </Match>
    </Switch>
  );
}
