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
      toast.success(
        "Withdrawal confirmed",
        `Sent ${withdrawal.amount.toFixed(2)} USDC to ${withdrawal.destination_address.slice(0, 6)}...${withdrawal.destination_address.slice(-4)}.`,
      );
    },
  });
}
