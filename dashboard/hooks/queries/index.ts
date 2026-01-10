// Position queries
export {
  usePositionsQuery,
  usePositionQuery,
  useClosePositionMutation,
  useOpenPositions,
  useCopyTradePositions,
} from './usePositionsQuery';

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
} from './useWalletsQuery';

// Discovery queries
export {
  useDiscoverWalletsQuery,
  useLiveTradesQuery,
  useDemoPnlSimulationQuery,
  useLeaderboardQuery,
} from './useDiscoverQuery';

// Recommendations queries
export {
  useRotationRecommendationsQuery,
  useDismissRecommendation,
  useAcceptRecommendation,
  type RotationRecommendation,
  type RecommendationType,
  type RecommendationReason,
  type Urgency,
} from './useRecommendationsQuery';
