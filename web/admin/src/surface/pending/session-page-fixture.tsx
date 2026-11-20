// 会话导航测试页：真实使用未决命令宿主、来源页登记、外壳回执与生成 Key 对话框。

import { createCell } from "@auction/web-shared/reactive/cell";
import { err, ok, type Result } from "@auction/web-shared/result";
import type { JSX } from "@solidjs/web";
import { createComponent, createMemo, createRoot, onCleanup, Show } from "solid-js";
import type { AdminWriteFailure } from "../../api/failure";
import { type PreparedJsonCommand, prepareJsonCommandOnce } from "../../api/transport";
import { AdminUserGeneratedKeyDialog, adminUserGeneratedKeyDelivery } from "../users/create";
import { type PendingCommandHost, PendingCommandHostContext, usePendingCommandGate } from ".";
import { PendingCommandNotices, PendingCommandPanel } from "./panel";
import { createCommandReload, usePendingCommands, usePendingCommandsWithGate } from "./sources";

/** 页面提交一次命令时由测试提供的写请求。 */
export type SessionPageSubmit = (
  prepared: PreparedJsonCommand,
) => Promise<Result<void, AdminWriteFailure>>;

/** 会话页的测试控制面。 */
export interface SessionPageRequests {
  readonly submit: SessionPageSubmit;
  readonly submitGenerated?: (() => Promise<Result<string, AdminWriteFailure>>) | undefined;
  submitted(entity: string, accepted: boolean): void;
  delivered(): void;
  keyDelivered(sourceAlive: boolean): void;
}

/** 会话导航页的属性。 */
export interface SessionPageProps {
  readonly host: PendingCommandHost;
  readonly requests: SessionPageRequests;
  readonly gatedDestination: boolean;
}

/** 外壳加来源页；切换导航会卸载来源页而保留会话宿主。 */
export function SessionPage(props: SessionPageProps): JSX.Element {
  return (
    <PendingCommandHostContext value={props.host}>
      <ProviderBody
        host={props.host}
        requests={props.requests}
        gatedDestination={props.gatedDestination}
      />
    </PendingCommandHostContext>
  );
}

/** 在宿主上下文内固定页面组件实例，避免重复求值重建页面状态。 */
function ProviderBody(props: {
  readonly host: PendingCommandHost;
  readonly requests: SessionPageRequests;
  readonly gatedDestination: boolean;
}): JSX.Element {
  const body = createMemo(() =>
    createComponent(SessionBody, {
      host: props.host,
      requests: props.requests,
      gatedDestination: props.gatedDestination,
    }),
  );
  return <>{body()}</>;
}

function SessionBody(props: {
  readonly host: PendingCommandHost;
  readonly requests: SessionPageRequests;
  readonly gatedDestination: boolean;
}): JSX.Element {
  const origin = createCell(true);
  let originPage: JSX.Element = null;
  let destinationPage: JSX.Element = null;
  let originRoot: (() => void) | undefined;
  let destinationRoot: (() => void) | undefined;

  const mountOrigin = (): void => {
    createRoot((dispose) => {
      originRoot = dispose;
      originPage = createComponent(PendingCommandHostContext, {
        value: props.host,
        get children() {
          const submitGenerated = props.requests.submitGenerated;
          return submitGenerated === undefined
            ? createComponent(OriginPage, { requests: props.requests })
            : createComponent(GeneratedKeySource, { requests: props.requests, submitGenerated });
        },
      });
    });
  };
  const mountDestination = (): void => {
    createRoot((dispose) => {
      destinationRoot = dispose;
      destinationPage = createComponent(PendingCommandHostContext, {
        value: props.host,
        get children() {
          return props.gatedDestination ? (
            createComponent(GatedPage, { requests: props.requests })
          ) : (
            <p>只读页面</p>
          );
        },
      });
    });
  };
  const toggle = (): void => {
    if (origin.peek()) {
      originRoot?.();
      originRoot = undefined;
      mountDestination();
      origin.set(false);
    } else {
      destinationRoot?.();
      destinationRoot = undefined;
      mountOrigin();
      origin.set(true);
    }
  };

  onCleanup(() => {
    try {
      originRoot?.();
    } finally {
      destinationRoot?.();
    }
  });
  mountOrigin();
  return (
    <>
      <button type="button" onClick={toggle}>
        切换测试路由
      </button>
      <PendingCommandNotices host={props.host} />
      {/* 条件表达式每次更新都会重新读取当前页；不能放进 Show/Switch 的惰性子树。 */}
      {origin() ? originPage : destinationPage}
    </>
  );
}

function OriginPage(props: { readonly requests: SessionPageRequests }): JSX.Element {
  const reload = createCommandReload(0n);
  const gate = usePendingCommandGate();
  const owner = usePendingCommandsWithGate(reload, gate, () => true);
  const success = createCell(false);
  const submitKey = (): void => {
    const entity = "user:fixture";
    props.requests.submitted(
      entity,
      owner.submitWithReceipt(
        entity,
        "生成 Key",
        async () => ok("已生成的测试 Key"),
        (key, sourceAlive) => {
          props.requests.keyDelivered(sourceAlive);
          return adminUserGeneratedKeyDelivery(key, false);
        },
        () => {},
      ),
    );
  };
  return (
    <>
      <button
        type="button"
        disabled={owner.contains("alert:fixture")}
        onClick={() => {
          const reason = "第一次提交的原因";
          const prepare = prepareJsonCommandOnce((commandId) => ({
            command_id: commandId,
            reason,
          }));
          props.requests.submitted(
            "alert:fixture",
            owner.submit(
              "alert:fixture",
              "测试静默",
              async (commandId) => {
                const prepared = prepare(commandId);
                return prepared.ok ? props.requests.submit(prepared.value) : err(prepared.error);
              },
              () => {
                props.requests.delivered();
                success.set(true);
              },
              () => {},
            ),
          );
        }}
      >
        发送测试命令
      </button>
      <button type="button" onClick={submitKey}>
        生成待确认 Key
      </button>
      <Show when={success()}>
        <p>来源页收到回执</p>
      </Show>
      <PendingCommandPanel owner={owner} />
      <Show when={owner.generatedKey()}>
        {(generated) => (
          <AdminUserGeneratedKeyDialog
            generated={generated().key}
            onClose={() => owner.acknowledgeKey(generated().commandId)}
          />
        )}
      </Show>
    </>
  );
}

function GeneratedKeySource(props: {
  readonly requests: SessionPageRequests;
  readonly submitGenerated: () => Promise<Result<string, AdminWriteFailure>>;
}): JSX.Element {
  const owner = usePendingCommands(createCommandReload(0n), () => true);
  return (
    <button
      type="button"
      onClick={() => {
        const entity = "create-user";
        props.requests.submitted(
          entity,
          owner.submitWithReceipt(
            entity,
            "测试创建用户",
            props.submitGenerated,
            (key, sourceAlive) => {
              props.requests.keyDelivered(sourceAlive);
              return adminUserGeneratedKeyDelivery(key, false);
            },
            () => {},
          ),
        );
      }}
    >
      发送测试命令
    </button>
  );
}

function GatedPage(props: { readonly requests: SessionPageRequests }): JSX.Element {
  const reload = createCommandReload(0n);
  const gate = usePendingCommandGate();
  const owner = usePendingCommandsWithGate(reload, gate, () => true);
  return (
    <>
      <button
        type="button"
        disabled={owner.anyBusy() || gate.blocked()}
        onClick={() => {
          props.requests.submitted(
            "world:fixture",
            owner.submit(
              "world:fixture",
              "测试世界操作",
              async () => ok(undefined),
              () => gate.holdResult(),
              () => {},
            ),
          );
        }}
      >
        交付一次性结果
      </button>
      <button type="button" onClick={() => gate.acknowledge()}>
        确认已查看
      </button>
    </>
  );
}
