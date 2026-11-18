// 代理表单在授权重读失败和重试期间保留其上一次确认的模式。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createRoot, flush } from "solid-js";

import type { OwnedProxy } from "../../../api/account";
import { activeOwnedProxy, useProxyDraftRead } from ".";

const ACTIVE_PROXY: OwnedProxy = {
  authorization_id: "authorization-1",
  auction_id: "auction-1",
  lot_id: "lot-1",
  lot_number: 1,
  authorization_sequence: 1,
  usage_number: 1,
  mode: "slow",
  maximum_amount: 150_000,
  quota: { used: 1, limit: 3 },
  transitions: [],
  status: "active",
  terminal_reason: null,
  state_version: 2,
  created_at: "2026-11-13T00:00:00Z",
  updated_at: "2026-11-13T00:00:00Z",
  equipment: { name: "Legendary Shield", category: "shield" },
  bid: null,
  invalid_reason: null,
  capabilities: { can_lower: true, waiting: true, at_maximum: false },
};

test("已挂载的创建表单经过读取失败和重试仍保留，重新确认后才可提交", () => {
  createRoot((dispose) => {
    try {
      const noProxy = ok(null);
      const snapshot = createCell<Result<OwnedProxy | null, string> | null>(noProxy);
      const draft = useProxyDraftRead(snapshot);
      expect(draft()).toEqual({ confirmed: noProxy, error: null, canSubmit: true });

      snapshot.set(err("network_error"));
      flush();
      expect(draft()).toEqual({ confirmed: noProxy, error: "network_error", canSubmit: false });

      snapshot.set(null);
      flush();
      expect(draft()).toEqual({ confirmed: noProxy, error: null, canSubmit: false });

      snapshot.set(noProxy);
      flush();
      expect(draft()).toEqual({ confirmed: noProxy, error: null, canSubmit: true });
    } finally {
      dispose();
    }
  });
});

test("降低上限表单在重读失败与重试期间沿用同一授权", () => {
  createRoot((dispose) => {
    try {
      const snapshot = createCell<Result<OwnedProxy | null, string> | null>(ok(ACTIVE_PROXY));
      const draft = useProxyDraftRead(snapshot);
      expect(activeOwnedProxy(draft().confirmed)).toBe(ACTIVE_PROXY);
      expect(draft().canSubmit).toBe(true);

      for (const result of [err("network_error"), null]) {
        snapshot.set(result);
        flush();
        expect(activeOwnedProxy(draft().confirmed)).toBe(ACTIVE_PROXY);
        expect(draft().canSubmit).toBe(false);
      }

      const confirmed = { ...ACTIVE_PROXY, maximum_amount: 140_000, state_version: 3 };
      snapshot.set(ok(confirmed));
      flush();
      expect(activeOwnedProxy(draft().confirmed)).toBe(confirmed);
      expect(draft().canSubmit).toBe(true);
    } finally {
      dispose();
    }
  });
});

test("首次授权读取失败时仍无可挂载的代理表单", () => {
  createRoot((dispose) => {
    try {
      const draft = useProxyDraftRead(() => err("network_error"));
      expect(draft()).toEqual({ confirmed: null, error: "network_error", canSubmit: false });
    } finally {
      dispose();
    }
  });
});
