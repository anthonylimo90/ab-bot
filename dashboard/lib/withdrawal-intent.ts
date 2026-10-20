export interface WithdrawalIntent { fingerprint: string; id: string }

// Keep the same economic intent through errors, retries and page reloads.
export function withdrawalIntent(
  storage: Pick<Storage, 'getItem' | 'setItem'>,
  fingerprint: string,
  previous: WithdrawalIntent | null = null,
): WithdrawalIntent {
  const key = 'ab-bot-withdrawal-intent';
  if (previous?.fingerprint === fingerprint) return previous;
  try {
    const saved = JSON.parse(storage.getItem(key) ?? 'null') as WithdrawalIntent | null;
    if (saved?.fingerprint === fingerprint && typeof saved.id === 'string') return saved;
  } catch { /* Missing or invalid persisted state starts a new intent. */ }
  const intent = { fingerprint, id: crypto.randomUUID() };
  storage.setItem(key, JSON.stringify(intent));
  return intent;
}
