// Framework-free WebSocket lifecycle: connect guard, jittered exponential backoff,
// keep-alive pings and clean replacement of dead sockets. Kept out of the React hook
// so every transition can be unit tested with a fake socket and mock timers.

export type SocketStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

// WebSocket.readyState values (a global WebSocket is not guaranteed in tests).
export const SOCKET_CONNECTING = 0;
export const SOCKET_OPEN = 1;

/** Close code the server uses when the session token is rejected or revoked. */
export const CLOSE_POLICY_VIOLATION = 1008;

export interface SocketLike {
  readyState: number;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

/**
 * Delay before reconnect attempt number `attempt` (0-based): exponential from
 * `baseMs`, capped at `maxMs`, with "equal jitter" so the result lies in
 * [ceiling / 2, ceiling). Clients that lost the server together spread out instead
 * of reconnecting in lockstep, and none retries faster than half the ceiling.
 */
export function reconnectDelayMs(
  attempt: number,
  options: { baseMs?: number; maxMs?: number; random?: () => number } = {},
): number {
  const { baseMs = 3000, maxMs = 30_000, random = Math.random } = options;
  // Bound the exponent so a very long outage cannot overflow 2 ** attempt to Infinity.
  const ceiling = Math.min(maxMs, baseMs * 2 ** Math.min(Math.max(attempt, 0), 30));
  return Math.round(ceiling / 2 + random() * (ceiling / 2));
}

export interface ReconnectingSocketOptions {
  url: string;
  /**
   * Sent as `{ action: 'authenticate', token }` the moment the socket opens. A
   * function is asked for a fresh token at every open (a stream token, issued
   * per connection); if it fails the socket is closed and retried.
   */
  authToken?: string | null | (() => Promise<string | null>);
  reconnect?: boolean;
  baseDelayMs?: number;
  maxDelayMs?: number;
  /** Consecutive failed attempts before giving up. Defaults to unlimited. */
  maxAttempts?: number;
  pingIntervalMs?: number;
  createSocket?: (url: string) => SocketLike;
  random?: () => number;
  onStatus?: (status: SocketStatus) => void;
  /** Raw frame payload from the *current* socket only. */
  onMessage?: (data: unknown) => void;
  onDisconnect?: () => void;
  onError?: (event: unknown) => void;
}

export interface ReconnectingSocket {
  /** Open a socket unless one is already open or connecting. */
  connect(): void;
  /** Close and stop reconnecting until `connect` / `resume` is called. */
  disconnect(): void;
  /** Drop the current socket and open a fresh one immediately, resetting backoff. */
  reconnectNow(): void;
  /** Tab became visible: reconnect now unless a socket is open or connecting. */
  resume(): void;
  /** Application-level success (server confirmed the subscription): reset backoff. */
  markConnected(): void;
  send(data: string): boolean;
  /** Tear everything down; the instance cannot be reused. */
  dispose(): void;
}

export function createReconnectingSocket(options: ReconnectingSocketOptions): ReconnectingSocket {
  const {
    url,
    authToken = null,
    reconnect = true,
    baseDelayMs = 3000,
    maxDelayMs = 30_000,
    maxAttempts = Infinity,
    pingIntervalMs = 30_000,
    createSocket = (target: string) => new WebSocket(target) as unknown as SocketLike,
    random = Math.random,
    onStatus,
    onMessage,
    onDisconnect,
    onError,
  } = options;

  let socket: SocketLike | null = null;
  let pingTimer: ReturnType<typeof setInterval> | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  let attempts = 0;
  // Set by disconnect() and by a policy close: no automatic reconnect until the
  // owner asks again (connect / resume / reconnectNow).
  let halted = false;
  let disposed = false;

  // Deliberately not a type predicate: a CLOSING/CLOSED socket is not "live" but
  // is still a socket we must release.
  const isLive = (candidate: SocketLike | null): boolean =>
    candidate !== null &&
    (candidate.readyState === SOCKET_OPEN || candidate.readyState === SOCKET_CONNECTING);

  function clearReconnectTimer() {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
  }

  function stopPing() {
    if (pingTimer !== null) {
      clearInterval(pingTimer);
      pingTimer = null;
    }
  }

  // Detach every handler first so a late event from a socket we replaced can never
  // schedule a reconnect, touch the ping timer or report status for the new socket.
  function discard(target: SocketLike) {
    target.onopen = null;
    target.onmessage = null;
    target.onclose = null;
    target.onerror = null;
    try {
      target.close();
    } catch {
      // Already closed.
    }
  }

  function scheduleReconnect() {
    if (disposed || halted || !reconnect || attempts >= maxAttempts) return;
    const delay = reconnectDelayMs(attempts, {
      baseMs: baseDelayMs,
      maxMs: maxDelayMs,
      random,
    });
    attempts += 1;
    clearReconnectTimer();
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delay);
  }

  function connect() {
    if (disposed) return;
    // A pending backoff timer is superseded by this explicit attempt.
    clearReconnectTimer();
    // OPEN and CONNECTING sockets are left alone. Replacing a CONNECTING socket
    // would leak it (and, once open, its ping timer).
    if (isLive(socket)) return;
    if (socket) {
      // A CLOSING or CLOSED leftover: release it and its ping timer before replacing.
      stopPing();
      discard(socket);
      socket = null;
    }
    halted = false;
    onStatus?.('connecting');

    let next: SocketLike;
    try {
      next = createSocket(url);
    } catch (error) {
      // A bad URL or blocked scheme will not fix itself, so do not retry.
      onStatus?.('error');
      console.error('WebSocket connection failed:', error);
      return;
    }
    socket = next;

    next.onopen = () => {
      if (socket !== next) return;
      const authenticate = (token: string | null) => {
        if (token && socket === next && next.readyState === SOCKET_OPEN) {
          next.send(JSON.stringify({ action: 'authenticate', token }));
        }
      };
      if (typeof authToken === 'function') {
        authToken().then(authenticate, (error) => {
          if (socket !== next) return;
          console.error('Could not get a token for the live stream:', error);
          // Closed, so onclose schedules a retry with backoff.
          next.close();
        });
      } else {
        authenticate(authToken);
      }
      stopPing();
      pingTimer = setInterval(() => {
        if (next.readyState === SOCKET_OPEN) {
          next.send(JSON.stringify({ action: 'ping' }));
        }
      }, pingIntervalMs);
    };

    next.onmessage = (event) => {
      if (socket !== next) return;
      onMessage?.(event.data);
    };

    next.onclose = (event) => {
      if (socket !== next) return;
      stopPing();
      socket = null;
      const policyViolation = event.code === CLOSE_POLICY_VIOLATION;
      onStatus?.(policyViolation ? 'error' : 'disconnected');
      onDisconnect?.();
      if (policyViolation && typeof authToken !== 'function') {
        // The server rejected our credentials; retrying with the same token only
        // repeats the rejection. A new token (re-render) or an explicit resume retries.
        halted = true;
        return;
      }
      // With a token provider the next attempt gets a fresh token (the last
      // one expired, or came too late); a session that has ended answers the
      // token request with 401, which signs the page out.
      scheduleReconnect();
    };

    next.onerror = (event) => {
      if (socket !== next) return;
      onStatus?.('error');
      onError?.(event);
    };
  }

  return {
    connect,

    disconnect() {
      halted = true;
      clearReconnectTimer();
      stopPing();
      // The close event (still attached) reports status and disconnection once.
      socket?.close();
    },

    reconnectNow() {
      if (disposed) return;
      attempts = 0;
      clearReconnectTimer();
      stopPing();
      if (socket) {
        discard(socket);
        socket = null;
      }
      connect();
    },

    resume() {
      if (disposed || isLive(socket)) return;
      attempts = 0;
      connect();
    },

    markConnected() {
      attempts = 0;
      onStatus?.('connected');
    },

    send(data) {
      if (socket && socket.readyState === SOCKET_OPEN) {
        socket.send(data);
        return true;
      }
      return false;
    },

    dispose() {
      disposed = true;
      clearReconnectTimer();
      stopPing();
      if (socket) {
        discard(socket);
        socket = null;
      }
    },
  };
}
