"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { queryKeys } from "@/lib/queryClient";
import { useToastStore } from "@/stores/toast-store";
import type { CreateWalletWithdrawalRequest, WalletWithdrawal } from "@/types/api";

export function useWalletBalanceQuery(address: string | null) {
  return useQuery({
    queryKey: queryKeys.wallets.balance(address),
    queryFn: () => api.getWalletBalance(address!),
    enabled: Boolean(address),
    staleTime: 30_000,
    refetchInterval: 60_000,
    placeholderData: (previousData) => previousData,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}

/** What one withdrawal from `address` could take now (owners and admins). */
export function useWithdrawalCapacityQuery(address: string | null, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.wallets.withdrawalCapacity(address),
    queryFn: () => api.getWithdrawalCapacity(address!),
    enabled: enabled && Boolean(address),
    staleTime: 30_000,
    refetchInterval: 60_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}

export function useWalletWithdrawalsQuery(limit = 10) {
  return useQuery({
    queryKey: queryKeys.wallets.withdrawals({ limit }),
    queryFn: () => api.listWalletWithdrawals(limit),
    staleTime: 30_000,
    refetchInterval: 60_000,
  });
}

export function useCreateWalletWithdrawalMutation() {
  const queryClient = useQueryClient();
  const toast = useToastStore();

  return useMutation({
    mutationFn: (params: CreateWalletWithdrawalRequest) =>
      api.createWalletWithdrawal(params),
    onSuccess: (withdrawal: WalletWithdrawal) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.wallets.all() });
      queryClient.invalidateQueries({ queryKey: queryKeys.account.all() });
      queryClient.invalidateQueries({ queryKey: queryKeys.runtime.serviceStatus() });
      if (withdrawal.status === "confirmed") {
        toast.success("Withdrawal confirmed", `Sent ${withdrawal.amount.toFixed(2)} USDC.`);
      } else if (withdrawal.status === "failed") {
        toast.error("Withdrawal failed", withdrawal.error ?? "Check withdrawal history before starting another transfer.");
      } else {
        toast.info("Withdrawal pending", "Check withdrawal history. Retry with the same request; do not start another transfer.");
      }
    },
  });
}
