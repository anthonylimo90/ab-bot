"use client";

import { useWorkspaceStore } from "@/stores/workspace-store";
import { useServiceStatusQuery } from "@/hooks/queries/useRiskQuery";

/**
 * The trading switches as the server applies them. A switch is on when the
 * workspace turns it on or the server's environment does; one the
 * environment turns on cannot be switched off from the dashboard. The
 * environment drives only the canonical workspace, the one service status
 * describes.
 */
export function useTradingSwitches() {
  const { currentWorkspace } = useWorkspaceStore();
  const { data: serviceStatus } = useServiceStatusQuery(currentWorkspace?.id);
  const environment =
    serviceStatus && serviceStatus.workspace_id === currentWorkspace?.id
      ? serviceStatus.environment_switches
      : undefined;
  const forced = {
    live_trading_enabled: Boolean(environment?.live_trading),
    arb_auto_execute: Boolean(environment?.arb_auto_execute),
    exit_handler_enabled: Boolean(environment?.exit_handler),
  };
  const saved = {
    live_trading_enabled: Boolean(currentWorkspace?.live_trading_enabled),
    arb_auto_execute: Boolean(currentWorkspace?.arb_auto_execute),
    exit_handler_enabled: Boolean(currentWorkspace?.exit_handler_enabled),
  };
  return {
    serviceStatus,
    /** Whether the environment's switches are known (service status read). */
    environmentKnown: environment !== undefined,
    /** The workspace's own switches. */
    saved,
    /** The switches the environment holds on. */
    forced,
    liveTrading: saved.live_trading_enabled || forced.live_trading_enabled,
    arbAutoExecute: saved.arb_auto_execute || forced.arb_auto_execute,
    exitHandler: saved.exit_handler_enabled || forced.exit_handler_enabled,
  };
}
