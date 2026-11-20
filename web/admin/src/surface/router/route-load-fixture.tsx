// 接入真实认证外壳，控制页面分块的交付和测试来源页提交的命令。

import { ThemeStateContext, useThemeEffects } from "@auction/web-shared/platform/theme";
import { err } from "@auction/web-shared/result";
import { pageComponent, type Router, RouterContext } from "@auction/web-shared/router";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import type { JSX } from "@solidjs/web";
import { type Component, createContext, onCleanup, useContext } from "solid-js";
import { prepareJsonCommandOnce, sendPreparedJsonWrite } from "../../api/transport";
import { preserveProperty } from "../../test-scope-fixture";
import { AdminShell } from "../auth";
import { useAdminSessionHost } from "../auth/shell";
import { type ConfigDraft, ConfigDraftContext } from "../config/draft";
import type { PendingCommandHost } from "../pending";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import { type DocumentDrafts, DocumentDraftsContext } from "../rules/model";
import { required } from "../solid-dom-fixture";
import { adminUserGeneratedKeyDelivery } from "../users/create";
import { ADMIN_PAGES } from "./index";

/** 测试来源页使用生产读取与写入宿主。 */
export const SOURCE_ROUTE = { name: "RouteLoadSource", pattern: "/route-load-source" };
/** 交给实际发送器的同源写请求，由用例控制回执到达时间。 */
export const WRITE_PATH = "/api/v1/admin/test-route-load-write";
/** 只在内存里的虚构一次性回执内容。 */
export const FIXTURE_KEY = "fixture-only-key";

type ReceiptKind = "ordinary" | "key";

interface MountedSource {
  readonly host: PendingCommandHost;
  readonly config: ConfigDraft;
  readonly documents: DocumentDrafts;
  send(kind: ReceiptKind): void;
}

/** 页面挂载、卸载以及当前会话提供方的观察记录。 */
export interface SourceObservation {
  readonly mounts: MountedSource[];
  unmounts: number;
}

const SourceContext = createContext<SourceObservation | null>(null);

function SourcePage(): JSX.Element {
  const observation = required(useContext(SourceContext), "测试来源页观察记录");
  const config = required(useContext(ConfigDraftContext), "运行配置草稿");
  const documents = required(useContext(DocumentDraftsContext), "站点文档草稿");
  const session = useAdminSessionHost();
  const commands = usePendingCommands(createCommandReload(0n), () => true);

  onCleanup(() => {
    observation.unmounts += 1;
  });
  observation.mounts.push({
    host: commands.host,
    config,
    documents,
    send(kind) {
      const prepare = prepareJsonCommandOnce((commandId) => ({
        command_id: commandId,
        value: "冻结的初始值",
      }));
      const submitted = commands.submitWithReceipt(
        "user:fixture",
        "测试变更",
        (commandId, signal) => {
          const body = prepare(commandId);
          if (!body.ok) return Promise.resolve(err(body.error));
          return sendPreparedJsonWrite(
            session,
            { method: "POST", url: WRITE_PATH, endpoint: "user-mutation" },
            body.value,
            signal,
          );
        },
        () =>
          kind === "key"
            ? adminUserGeneratedKeyDelivery(FIXTURE_KEY, false)
            : { kind: "receipt", receipt: { kind: "text", text: "测试成功回执" } },
        () => {},
      );
      if (!submitted) throw new Error("测试命令未提交");
    },
  });
  return <h1 id="route-load-source">测试来源页</h1>;
}

/** 安装测试来源页，生产路由保持原样。 */
export function installSourcePage(): () => void {
  const restore = preserveProperty(ADMIN_PAGES, SOURCE_ROUTE.name);
  Object.assign(ADMIN_PAGES, { [SOURCE_ROUTE.name]: pageComponent(SourcePage) });
  return restore;
}

/** 拦住实际注册的动态页面入口。成功时继续加载真正的页面模块。 */
export function deferAdminPage(name: string) {
  const page = required(ADMIN_PAGES[name], name);
  if (page.kind !== "lazy") throw new Error(`测试目标不是按需加载的页面：${name}`);
  const restore = preserveProperty(ADMIN_PAGES, name);
  const firstLoad = Promise.withResolvers<boolean>();
  const firstCompleted = Promise.withResolvers<"loaded" | "failed">();
  let attempts = 0;
  Object.assign(ADMIN_PAGES, {
    [name]: {
      kind: "lazy",
      async load() {
        attempts += 1;
        if (attempts !== 1) return page.load();
        try {
          if (!(await firstLoad.promise)) {
            throw new Error("测试分块加载失败");
          }
          const component = await page.load();
          firstCompleted.resolve("loaded");
          return component;
        } catch (error) {
          firstCompleted.resolve("failed");
          throw error;
        }
      },
    },
  });
  return {
    attempts: () => attempts,
    release() {
      firstLoad.resolve(true);
      return firstCompleted.promise;
    },
    fail() {
      firstLoad.resolve(false);
      return firstCompleted.promise;
    },
    restore() {
      firstLoad.resolve(true);
      restore();
    },
  };
}

/** 挂载真实认证外壳，并提供可发起命令的测试来源页。 */
export function RouteLoadHarness(props: {
  readonly router: Router;
  readonly locale: LocaleSession;
  readonly observation: SourceObservation;
}): JSX.Element {
  return (
    <RouteLoadProviders locale={props.locale}>
      <SourceContext value={props.observation}>
        <RouterContext value={props.router}>
          <AdminShell />
        </RouterContext>
      </SourceContext>
    </RouteLoadProviders>
  );
}

/** 首次设置由生产路由宿主选择布局及页面。 */
export function SetupHarness(props: {
  readonly locale: LocaleSession;
  readonly routerHost: Component;
}): JSX.Element {
  const RouterHost = props.routerHost;
  return (
    <RouteLoadProviders locale={props.locale}>
      <RouterHost />
    </RouteLoadProviders>
  );
}

function RouteLoadProviders(props: {
  readonly locale: LocaleSession;
  readonly children: JSX.Element;
}): JSX.Element {
  const theme = useThemeEffects();
  return (
    <LocaleSessionContext value={props.locale}>
      <ThemeStateContext value={theme}>{props.children}</ThemeStateContext>
    </LocaleSessionContext>
  );
}
