// Position queries
export {
  usePositionsQuery,
  usePositionQuery,
  useClosePositionMutation,
  useOpenPositions,
  useCopyTradePositions,
} from "./usePositionsQuery";

// Wallet queries
export {
  useWalletsQuery,
  useWalletQuery,
  useWalletMetricsQuery,
  useTrackWalletMutation,
  useUntrackWalletMutation,
  useUpdateWalletMutation,
  useRosterWallets,
  useBenchWallets,
} from "./useWalletsQuery";

// Discovery queries
export {
  useDiscoverWalletsQuery,
  useLiveTradesQuery,
  useLeaderboardQuery,
} from "./useDiscoverQuery";

// Recommendations queries
export {
  useRotationRecommendationsQuery,
  useDismissRecommendation,
  useAcceptRecommendation,
  type RotationRecommendation,
  type RecommendationType,
  type RecommendationReason,
  type Urgency,
} from "./useRecommendationsQuery";

// Optimizer queries
export {
  useOptimizerStatusQuery,
  useRotationHistoryQuery,
  useTriggerOptimizationMutation,
  useAcknowledgeRotationMutation,
  useUnacknowledgedRotationCount,
} from "./useOptimizerQuery";

// History queries
export { useClosedPositionsQuery } from "./useHistoryQuery";

// Risk monitoring queries
export {
  useRiskStatusQuery,
  useManualTripMutation,
  useResetCircuitBreakerMutation,
} from "./useRiskQuery";

// Allocation queries
export {
  useAllocationsQuery,
  useActiveAllocationsQuery,
  useBenchAllocationsQuery,
  useAddAllocationMutation,
  useUpdateAllocationMutation,
  usePromoteAllocationMutation,
  useDemoteAllocationMutation,
  useRemoveAllocationMutation,
  usePinAllocationMutation,
  useUnpinAllocationMutation,
} from "./useAllocationsQuery";
