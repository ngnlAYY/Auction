// 运行配置测试的契约样本视图：只做结构与字段的运行期检查，不改写样本文件。

import { jsonObject, parseJsonObject } from "../api-request-fixture";

/** 读取、预校验与保存共用的投影；可选的扩展字段按样本是否存在区分。 */
export interface RuntimeConfigProjection {
  readonly [key: string]: unknown;
  data: RuntimeConfigProjectionData;
}

/** 样本的 `data`：文档、fingerprint 与可选扩展投影。 */
export interface RuntimeConfigProjectionData {
  readonly [key: string]: unknown;
  document: Record<string, unknown>;
  fingerprint: string;
  restart_required: boolean;
  equipment_taxonomy?: unknown;
  worker_restart_state?: unknown;
}

/** 运行配置测试的样本读取器；文本由测试文件自己从契约夹具目录读取。 */
export interface RuntimeConfigFixtures {
  /** 读取并检查一份运行配置样本。 */
  runtimeProjection(name: string): RuntimeConfigProjection;
  /** 默认后端投影；Rust 的 `default_response()` 同样取自这份样本。 */
  defaultProjection(): RuntimeConfigProjection;
  /** 默认样本里的配置文档。 */
  defaultDocument(): Record<string, unknown>;
  /** 默认样本里的运行参数区块。 */
  defaultOperations(): Record<string, unknown>;
  /** Worker 重启状态只从 Worker 投影样本取值，不在测试里手写。 */
  workerRestartState(): unknown;
  /** 以完整默认投影为底，单独加入 Worker 投影；其余编辑器投影保持齐全。 */
  withWorkerState(state: unknown): RuntimeConfigProjection;
  /** 深拷贝样本，避免用例之间的修改互相影响。 */
  cloneProjection(projection: RuntimeConfigProjection): RuntimeConfigProjection;
}

/** 绑定测试文件的夹具读取函数；模块本身不访问文件系统。 */
export function createRuntimeConfigFixtures(
  readText: (name: string) => string,
): RuntimeConfigFixtures {
  function runtimeProjection(name: string): RuntimeConfigProjection {
    const root = parseJsonObject(readText(name), `${name}.json`);
    const data = jsonObject(root.data, `${name}.data`);
    const document = jsonObject(data.document, `${name}.data.document`);
    if (typeof data.fingerprint !== "string") {
      throw new Error(`${name}.data.fingerprint 必须是字符串`);
    }
    if (typeof data.restart_required !== "boolean") {
      throw new Error(`${name}.data.restart_required 必须是布尔值`);
    }
    return {
      ...root,
      data: {
        ...data,
        document,
        fingerprint: data.fingerprint,
        restart_required: data.restart_required,
      },
    };
  }

  function defaultProjection(): RuntimeConfigProjection {
    return runtimeProjection("runtime-config-default");
  }

  function defaultDocument(): Record<string, unknown> {
    return defaultProjection().data.document;
  }

  function defaultOperations(): Record<string, unknown> {
    return jsonObject(defaultDocument().operations, "runtime-config-default.operations");
  }

  function workerRestartState(): unknown {
    const projection = runtimeProjection("runtime-config-worker-restart-state");
    if (!Object.hasOwn(projection.data, "worker_restart_state")) {
      throw new Error("Worker 投影样本缺少 worker_restart_state");
    }
    return projection.data.worker_restart_state;
  }

  function withWorkerState(state: unknown): RuntimeConfigProjection {
    const projection = defaultProjection();
    projection.data.worker_restart_state = state;
    return projection;
  }

  function cloneProjection(projection: RuntimeConfigProjection): RuntimeConfigProjection {
    return structuredClone(projection);
  }

  return {
    runtimeProjection,
    defaultProjection,
    defaultDocument,
    defaultOperations,
    workerRestartState,
    withWorkerState,
    cloneProjection,
  };
}

export { jsonObjectAt, removeAt, setAt, valueAt } from "../api-request-fixture";
