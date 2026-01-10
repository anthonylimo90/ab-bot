import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '@/lib/api';

export type RecommendationType = 'demote' | 'promote' | 'alert';
export type RecommendationReason =
  | 'alpha_decay'
  | 'martingale_pattern'
  | 'strategy_drift'
  | 'honeypot_warning'
  | 'outperforming'
  | 'high_risk'
  | 'consistent_losses';
export type Urgency = 'low' | 'medium' | 'high';

export interface RotationRecommendation {
  id: string;
  type: RecommendationType;
  wallet_address: string;
  wallet_label?: string;
  reason: RecommendationReason;
  evidence: string[];
  urgency: Urgency;
  suggested_action: string;
  created_at: string;
}

interface RecommendationsParams {
  urgency?: Urgency;
  limit?: number;
}

async function fetchRotationRecommendations(
  params: RecommendationsParams
): Promise<RotationRecommendation[]> {
  const searchParams = new URLSearchParams();
  if (params.urgency) searchParams.set('urgency', params.urgency);
  if (params.limit) searchParams.set('limit', params.limit.toString());

  const query = searchParams.toString();
  const url = `/api/v1/recommendations/rotation${query ? `?${query}` : ''}`;
  return api.get<RotationRecommendation[]>(url);
}

async function dismissRecommendation(id: string): Promise<void> {
  await api.post(`/api/v1/recommendations/${id}/dismiss`, {});
}

async function acceptRecommendation(id: string): Promise<void> {
  await api.post(`/api/v1/recommendations/${id}/accept`, {});
}

export function useRotationRecommendationsQuery(params: RecommendationsParams = {}) {
  return useQuery({
    queryKey: ['rotation-recommendations', params],
    queryFn: () => fetchRotationRecommendations(params),
    staleTime: 30 * 1000, // 30 seconds
    refetchInterval: 60 * 1000, // Refetch every minute
  });
}

export function useDismissRecommendation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: dismissRecommendation,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['rotation-recommendations'] });
    },
  });
}

export function useAcceptRecommendation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: acceptRecommendation,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['rotation-recommendations'] });
    },
  });
}
