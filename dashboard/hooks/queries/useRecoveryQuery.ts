"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";

export function useRecoveryPreviewQuery(
  workspaceId?: string | null,
  enabled = true,
) {
  return useQuery({
    queryKey: ["recovery", "preview", workspaceId],
    queryFn: () => api.getRecoveryPreview(workspaceId!),
    enabled: Boolean(workspaceId) && enabled,
    staleTime: 10_000,
  });
}

export function useRefreshRecoveryPreviewMutation(workspaceId?: string | null) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.getRecoveryPreview(workspaceId!, true),
    retry: false,
    onSuccess: (data) => queryClient.setQueryData(["recovery", "preview", workspaceId], data),
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
          queryKey: queryKeys.recovery.preview(),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.account.all(),
        }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.runtime.serviceStatus(),
        }),
      ]);
    },
  });
}
