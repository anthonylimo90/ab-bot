export interface WithdrawalIntent { fingerprint: string; id: string }

export const WITHDRAWAL_INTENT_KEY = 'ab-bot-withdrawal-intent';

/**
 * A withdrawal intent ends when the server reports a terminal outcome: confirmed,
 * or failed (a retry must then create a new attempt with a new request id). It
 * is kept while the outcome is pending, submitted or unknown so that a retry
 * reuses the same client request id and can never send funds twice.
 */
export function isWithdrawalIntentSettled(status: string | null | undefined): boolean {
  const normalized = (status ?? '').trim().toLowerCase();
  return normalized === 'confirmed' || normalized === 'failed';
}

export function clearWithdrawalIntent(storage: Pick<Storage, 'removeItem'>): void {
  try {
    storage.removeItem(WITHDRAWAL_INTENT_KEY);
  } catch { /* Unavailable storage cannot hold a stale intent either. */ }
}

// Keep the same economic intent through errors, retries and page reloads.
export function withdrawalIntent(
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  fingerprint: string,
  previous: WithdrawalIntent | null = null,
): WithdrawalIntent {
  const key = WITHDRAWAL_INTENT_KEY;
  if (previous?.fingerprint === fingerprint) return previous;
  try {
    const saved = JSON.parse(storage.getItem(key) ?? 'null') as WithdrawalIntent | null;
    if (saved?.fingerprint === fingerprint && typeof saved.id === 'string') return saved;
  } catch { /* Missing or invalid persisted state starts a new intent. */ }
  const intent = { fingerprint, id: crypto.randomUUID() };
  storage.setItem(key, JSON.stringify(intent));
  return intent;
}
