"use client";

import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";

export function useWalletBalanceQuery(address: string | null) {
  return useQuery({
    queryKey: ["wallet-balance", address],
    queryFn: () => api.getWalletBalance(address!),
    enabled: Boolean(address),
    staleTime: 30_000,
    refetchInterval: 60_000,
    placeholderData: (previousData) => previousData,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
