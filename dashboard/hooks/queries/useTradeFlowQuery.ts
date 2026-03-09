"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import type {
  CreateLearningModelRequest,
  CreateLearningRolloutRequest,
  UpdateLearningModelRequest,
  UpdateLearningRolloutRequest,
} from "@/types/api";

type TradeFlowParams = {
  from?: string;
  to?: string;
  strategy?: string;
  limit?: number;
};

export function useTradeFlowSummaryQuery(params?: TradeFlowParams) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.summary(params),
    queryFn: () => api.getTradeFlowSummary(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useTradeFlowJourneysQuery(params?: TradeFlowParams) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.journeys(params),
    queryFn: () => api.getTradeFlowJourneys(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useMarketTradeFlowQuery(
  marketId: string,
  params?: TradeFlowParams,
) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.market(marketId, params),
    queryFn: () => api.getMarketTradeFlow(marketId, params),
    enabled: Boolean(marketId),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useArbExecutionTelemetryQuery(
  params?: Omit<TradeFlowParams, "strategy">,
) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.arbExecutionTelemetry(params),
    queryFn: () => api.getArbExecutionTelemetry(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useLearningOverviewQuery(
  params?: Omit<TradeFlowParams, "strategy">,
) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.learningOverview(params),
    queryFn: () => api.getLearningOverview(params),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useLearningRolloutDetailQuery(
  rolloutId: string | undefined,
  params?: { limit?: number },
) {
  return useQuery({
    queryKey: queryKeys.tradeFlow.learningRolloutDetail(rolloutId ?? "", params),
    queryFn: () => api.getLearningRolloutDetail(rolloutId!, params),
    enabled: Boolean(rolloutId),
    staleTime: 15_000,
    refetchInterval: 30_000,
  });
}

export function useCreateLearningRolloutMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (params: CreateLearningRolloutRequest) =>
      api.adminCreateLearningRollout(params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
    },
  });
}

export function useCreateLearningModelMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (params: CreateLearningModelRequest) =>
      api.adminCreateLearningModel(params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
    },
  });
}

export function useUpdateLearningModelMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      modelId,
      params,
    }: {
      modelId: string;
      params: UpdateLearningModelRequest;
    }) => api.adminUpdateLearningModel(modelId, params),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
    },
  });
}

export function useLearningModelStatusMutation(
  action: "activate" | "disable" | "retire",
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (modelId: string) => {
      switch (action) {
        case "activate":
          return api.adminActivateLearningModel(modelId);
        case "disable":
          return api.adminDisableLearningModel(modelId);
        case "retire":
          return api.adminRetireLearningModel(modelId);
      }
    },
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
    },
  });
}

export function useUpdateLearningRolloutMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      rolloutId,
      params,
    }: {
      rolloutId: string;
      params: UpdateLearningRolloutRequest;
    }) => api.adminUpdateLearningRollout(rolloutId, params),
    onSuccess: (_data, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.learningRolloutDetail(variables.rolloutId),
      });
    },
  });
}

export function useRolloutStatusActionMutation(action: "pause" | "resume" | "complete") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (rolloutId: string) => {
      switch (action) {
        case "pause":
          return api.adminPauseLearningRollout(rolloutId);
        case "resume":
          return api.adminResumeLearningRollout(rolloutId);
        case "complete":
          return api.adminCompleteLearningRollout(rolloutId);
      }
    },
    onSuccess: (_data, rolloutId) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.all(),
      });
      queryClient.invalidateQueries({
        queryKey: queryKeys.tradeFlow.learningRolloutDetail(rolloutId),
      });
    },
  });
}
