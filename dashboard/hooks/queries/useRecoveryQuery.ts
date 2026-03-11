"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

export function useRecoveryPreviewQuery(
  workspaceId?: string | null,
  enabled = true,
) {
  return useQuery({
    queryKey: workspaceId
      ? queryKeys.recovery.preview(workspaceId)
      : ["recovery", "preview", "disabled"],
    queryFn: () => api.getRecoveryPreview(workspaceId!),
    enabled: Boolean(workspaceId) && enabled,
    staleTime: 10_000,
  });
}

export function useRunRecoveryMutation(workspaceId?: string | null) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => api.runRecovery(workspaceId!),
    onSuccess: async () => {
      if (!workspaceId) return;

      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: queryKeys.recovery.preview(workspaceId),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.account.summary(workspaceId),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.account.history(workspaceId),
        }),
        queryClient.invalidateQueries({
          queryKey: ["service-status", workspaceId],
        }),
        queryClient.invalidateQueries({
          queryKey: ["workspace", workspaceId, "service-status"],
        }),
      ]);
    },
  });
}
