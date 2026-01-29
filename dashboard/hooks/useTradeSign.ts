'use client';

import { useState, useCallback } from 'react';
import { useAccount, useSignTypedData } from 'wagmi';
import api from '@/lib/api';

// EIP-712 type definitions matching backend
interface Eip712Domain {
  name: string;
  version: string;
  chainId: number;
  verifyingContract: string;
}

interface Eip712Order {
  salt: string;
  maker: string;
  signer: string;
  taker: string;
  tokenId: string;
  makerAmount: string;
  takerAmount: string;
  expiration: string;
  nonce: string;
  feeRateBps: string;
  side: number;
  signatureType: number;
}

interface Eip712TypedData {
  types: {
    EIP712Domain: Array<{ name: string; type: string }>;
    Order: Array<{ name: string; type: string }>;
  };
  primaryType: string;
  domain: Eip712Domain;
  message: Eip712Order;
}

interface OrderSummary {
  side: string;
  outcome: string;
  price: string;
  size: string;
  total_cost: string;
  potential_payout: string;
}

interface PrepareOrderResponse {
  pending_order_id: string;
  typed_data: Eip712TypedData;
  expires_at: string;
  summary: OrderSummary;
}

interface SubmitOrderResponse {
  success: boolean;
  order_id?: string;
  message: string;
  tx_hash?: string;
}

interface PrepareOrderParams {
  tokenId: string;
  side: 'BUY' | 'SELL';
  price: number;
  size: number;
  negRisk?: boolean;
  expiresInSecs?: number;
}

type TradeSignState = 'idle' | 'preparing' | 'awaiting_signature' | 'submitting' | 'success' | 'error';

interface UseTradeSignReturn {
  // State
  state: TradeSignState;
  error: string | null;
  pendingOrder: PrepareOrderResponse | null;
  submittedOrder: SubmitOrderResponse | null;

  // Actions
  prepareAndSign: (params: PrepareOrderParams) => Promise<SubmitOrderResponse | null>;
  reset: () => void;

  // Wallet state
  isConnected: boolean;
  address: string | undefined;
}

export function useTradeSign(): UseTradeSignReturn {
  const { address, isConnected } = useAccount();
  const { signTypedDataAsync } = useSignTypedData();

  const [state, setState] = useState<TradeSignState>('idle');
  const [error, setError] = useState<string | null>(null);
  const [pendingOrder, setPendingOrder] = useState<PrepareOrderResponse | null>(null);
  const [submittedOrder, setSubmittedOrder] = useState<SubmitOrderResponse | null>(null);

  const reset = useCallback(() => {
    setState('idle');
    setError(null);
    setPendingOrder(null);
    setSubmittedOrder(null);
  }, []);

  const prepareAndSign = useCallback(
    async (params: PrepareOrderParams): Promise<SubmitOrderResponse | null> => {
      if (!address || !isConnected) {
        setError('Wallet not connected');
        setState('error');
        return null;
      }

      try {
        // Step 1: Prepare order
        setState('preparing');
        setError(null);

        const prepareResponse = await api.post<PrepareOrderResponse>('/api/v1/orders/prepare', {
          token_id: params.tokenId,
          side: params.side,
          price: params.price,
          size: params.size,
          maker_address: address,
          neg_risk: params.negRisk || false,
          expires_in_secs: params.expiresInSecs || 3600,
        });

        setPendingOrder(prepareResponse);

        // Step 2: Sign typed data with MetaMask
        setState('awaiting_signature');

        const { typed_data } = prepareResponse;

        // Build the domain and types for wagmi's signTypedData
        const domain = {
          name: typed_data.domain.name,
          version: typed_data.domain.version,
          chainId: BigInt(typed_data.domain.chainId),
          verifyingContract: typed_data.domain.verifyingContract as `0x${string}`,
        };

        const types = {
          Order: [
            { name: 'salt', type: 'uint256' },
            { name: 'maker', type: 'address' },
            { name: 'signer', type: 'address' },
            { name: 'taker', type: 'address' },
            { name: 'tokenId', type: 'uint256' },
            { name: 'makerAmount', type: 'uint256' },
            { name: 'takerAmount', type: 'uint256' },
            { name: 'expiration', type: 'uint256' },
            { name: 'nonce', type: 'uint256' },
            { name: 'feeRateBps', type: 'uint256' },
            { name: 'side', type: 'uint8' },
            { name: 'signatureType', type: 'uint8' },
          ],
        } as const;

        const message = {
          salt: BigInt(typed_data.message.salt),
          maker: typed_data.message.maker as `0x${string}`,
          signer: typed_data.message.signer as `0x${string}`,
          taker: typed_data.message.taker as `0x${string}`,
          tokenId: BigInt(typed_data.message.tokenId),
          makerAmount: BigInt(typed_data.message.makerAmount),
          takerAmount: BigInt(typed_data.message.takerAmount),
          expiration: BigInt(typed_data.message.expiration),
          nonce: BigInt(typed_data.message.nonce),
          feeRateBps: BigInt(typed_data.message.feeRateBps),
          side: typed_data.message.side,
          signatureType: typed_data.message.signatureType,
        };

        const signature = await signTypedDataAsync({
          domain,
          types,
          primaryType: 'Order',
          message,
        });

        // Step 3: Submit signed order
        setState('submitting');

        const submitResponse = await api.post<SubmitOrderResponse>('/api/v1/orders/submit', {
          pending_order_id: prepareResponse.pending_order_id,
          signature,
        });

        setSubmittedOrder(submitResponse);
        setState('success');

        return submitResponse;
      } catch (err) {
        const errorMessage = err instanceof Error ? err.message : 'Unknown error';
        setError(errorMessage);
        setState('error');

        // If user rejected signature, provide clearer message
        if (errorMessage.includes('User rejected') || errorMessage.includes('user rejected')) {
          setError('Transaction signature was rejected');
        }

        return null;
      }
    },
    [address, isConnected, signTypedDataAsync]
  );

  return {
    state,
    error,
    pendingOrder,
    submittedOrder,
    prepareAndSign,
    reset,
    isConnected,
    address,
  };
}
