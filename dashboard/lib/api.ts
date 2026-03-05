import type {
  Position,
  PositionStatus,
  Market,
  Orderbook,
  Order,
  PlaceOrderRequest,
  BacktestParams,
  BacktestResult,
  HealthResponse,
  ApiError,
  User,
  WalletUser,
  AuthResponse,
  ConnectedWallet,
  StoreWalletRequest,
  UserListItem,
  CreateUserRequest,
  UpdateUserRequest,
  WorkspaceListItem,
  Workspace,
  WorkspaceMember,
  WorkspaceInvite,
  CreateWorkspaceRequest,
  UpdateWorkspaceRequest,
  UpdateOpportunitySelectionRequest,
  CreateInviteRequest,
  InviteInfo,
  AcceptInviteRequest,
  AcceptInviteResponse,
  WorkspaceRole,
  ServiceStatus,
  DynamicTunerStatus,
  DynamicConfigHistoryEntry,
  Activity,
  RiskStatus,
  CircuitBreakerStatus,
  CircuitBreakerConfig,
  MarketRegimeResponse,
  FlowFeatureResponse,
  RecentSignalResponse,
  StrategyPerformanceResponse,
  MarketMetadataResponse,
} from "@/types/api";

const API_BASE_URL = process.env.NEXT_PUBLIC_API_URL || "http://localhost:3001";

export class ApiHttpError extends Error {
  constructor(
    public statusCode: number,
    message: string,
    public code?: string,
  ) {
    super(message);
    this.name = "ApiHttpError";
  }

  get isUnauthorized() {
    return this.statusCode === 401;
  }
  get isForbidden() {
    return this.statusCode === 403;
  }
  get isNotFound() {
    return this.statusCode === 404;
  }
  get isRateLimited() {
    return this.statusCode === 429;
  }
  get isServerError() {
    return this.statusCode >= 500;
  }
}

/**
 * Coerce Decimal-serialized string fields on a Position to real numbers.
 *
 * rust_decimal serializes Decimal as a JSON string (e.g. "0.45") by default.
 * TypeScript's `response.json()` preserves strings, but our Position interface
 * declares these fields as `number`. This helper ensures runtime values match
 * the declared types so that `.toFixed()`, arithmetic, etc. work correctly.
 */
function parsePosition(raw: Position): Position {
  return {
    ...raw,
    quantity: Number(raw.quantity),
    entry_price: Number(raw.entry_price),
    current_price: Number(raw.current_price),
    unrealized_pnl: Number(raw.unrealized_pnl),
    unrealized_pnl_pct: Number(raw.unrealized_pnl_pct),
    stop_loss: raw.stop_loss != null ? Number(raw.stop_loss) : undefined,
    take_profit: raw.take_profit != null ? Number(raw.take_profit) : undefined,
    realized_pnl:
      raw.realized_pnl != null ? Number(raw.realized_pnl) : undefined,
  };
}

function parsePerformance(raw: StrategyPerformanceResponse): StrategyPerformanceResponse {
  return { ...raw, net_pnl: Number(raw.net_pnl), avg_pnl: Number(raw.avg_pnl) };
}

function parseFlowFeature(raw: FlowFeatureResponse): FlowFeatureResponse {
  return {
    ...raw,
    buy_volume: Number(raw.buy_volume),
    sell_volume: Number(raw.sell_volume),
    net_flow: Number(raw.net_flow),
    imbalance_ratio: Number(raw.imbalance_ratio),
    smart_money_flow: Number(raw.smart_money_flow),
  };
}

class ApiClient {
  private baseUrl: string;
  private token?: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  setToken(token: string) {
    this.token = token;
  }

  clearToken() {
    this.token = undefined;
  }

  // Generic HTTP methods for flexibility
  async get<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint);
  }

  async post<T>(endpoint: string, body: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "POST",
      body: JSON.stringify(body),
    });
  }

  async put<T>(endpoint: string, body: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  }

  async delete<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, {
      method: "DELETE",
    });
  }

  private async request<T>(
    endpoint: string,
    options: RequestInit = {},
    requestOptions?: {
      auth?: boolean;
    },
  ): Promise<T> {
    const headers: HeadersInit = {
      "Content-Type": "application/json",
      ...options.headers,
    };

    if (requestOptions?.auth !== false && this.token) {
      (headers as Record<string, string>)["Authorization"] =
        `Bearer ${this.token}`;
    }

    const response = await fetch(`${this.baseUrl}${endpoint}`, {
      ...options,
      headers,
    });

    if (!response.ok) {
      const error: ApiError = await response.json().catch(() => ({
        code: "UNKNOWN_ERROR",
        message: `HTTP ${response.status}`,
      }));
      throw new ApiHttpError(
        response.status,
        error.message || `HTTP ${response.status}`,
        error.code,
      );
    }

    // Handle 204 No Content and 202 Accepted (no body)
    if (response.status === 204 || response.status === 202) {
      return undefined as T;
    }

    return response.json();
  }

  // Health Check
  async healthCheck(): Promise<HealthResponse> {
    return this.request<HealthResponse>("/health");
  }

  async readyCheck(): Promise<HealthResponse> {
    return this.request<HealthResponse>("/ready");
  }

  // Auth
  async login(email: string, password: string): Promise<AuthResponse> {
    const response = await this.request<AuthResponse>("/api/v1/auth/login", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
    this.setToken(response.token);
    return response;
  }

  async refreshToken(): Promise<AuthResponse> {
    const response = await this.request<AuthResponse>("/api/v1/auth/refresh", {
      method: "POST",
    });
    this.setToken(response.token);
    return response;
  }

  async getCurrentUser(): Promise<User> {
    return this.request<User>("/api/v1/auth/me");
  }

  async forgotPassword(email: string): Promise<{ message: string }> {
    return this.request<{ message: string }>("/api/v1/auth/forgot-password", {
      method: "POST",
      body: JSON.stringify({ email }),
    });
  }

  async resetPassword(
    token: string,
    password: string,
  ): Promise<{ message: string }> {
    return this.request<{ message: string }>("/api/v1/auth/reset-password", {
      method: "POST",
      body: JSON.stringify({ token, password }),
    });
  }

  // Wallet Authentication (SIWE)
  async walletChallenge(address: string): Promise<{
    message: string;
    nonce: string;
    expires_at: string;
  }> {
    return this.request<{ message: string; nonce: string; expires_at: string }>(
      "/api/v1/auth/wallet/challenge",
      {
        method: "POST",
        body: JSON.stringify({ address }),
      },
    );
  }

  async walletVerify(
    message: string,
    signature: string,
  ): Promise<{
    token: string;
    user: WalletUser;
    is_new_user: boolean;
  }> {
    const response = await this.request<{
      token: string;
      user: WalletUser;
      is_new_user: boolean;
    }>("/api/v1/auth/wallet/verify", {
      method: "POST",
      body: JSON.stringify({ message, signature }),
    });
    this.setToken(response.token);
    return response;
  }

  async walletLink(
    message: string,
    signature: string,
  ): Promise<{ message: string; wallet_address: string }> {
    return this.request<{ message: string; wallet_address: string }>(
      "/api/v1/auth/wallet/link",
      {
        method: "POST",
        body: JSON.stringify({ message, signature }),
      },
    );
  }

  // Markets
  async getMarkets(params?: {
    category?: string;
    active?: boolean;
    min_volume?: number;
    limit?: number;
    offset?: number;
  }): Promise<Market[]> {
    const searchParams = new URLSearchParams();
    if (params?.category) searchParams.set("category", params.category);
    if (params?.active !== undefined)
      searchParams.set("active", String(params.active));
    if (params?.min_volume)
      searchParams.set("min_volume", String(params.min_volume));
    if (params?.limit) searchParams.set("limit", String(params.limit));
    if (params?.offset) searchParams.set("offset", String(params.offset));
    const query = searchParams.toString();
    return this.request<Market[]>(`/api/v1/markets${query ? `?${query}` : ""}`);
  }

  async getMarket(marketId: string): Promise<Market> {
    return this.request<Market>(`/api/v1/markets/${marketId}`);
  }

  async getOrderbook(marketId: string): Promise<Orderbook> {
    return this.request<Orderbook>(`/api/v1/markets/${marketId}/orderbook`);
  }

  // Positions
  async getPositions(params?: {
    market_id?: string;
    outcome?: "yes" | "no";
    status?: PositionStatus;
    limit?: number;
    offset?: number;
  }): Promise<Position[]> {
    const searchParams = new URLSearchParams();
    if (params?.market_id) searchParams.set("market_id", params.market_id);
    if (params?.outcome) searchParams.set("outcome", params.outcome);
    if (params?.status) searchParams.set("status", params.status);
    if (params?.limit) searchParams.set("limit", String(params.limit));
    if (params?.offset) searchParams.set("offset", String(params.offset));
    const query = searchParams.toString();
    const raw = await this.request<Position[]>(
      `/api/v1/positions${query ? `?${query}` : ""}`,
    );
    return raw.map(parsePosition);
  }

  async getPosition(positionId: string): Promise<Position> {
    const raw = await this.request<Position>(
      `/api/v1/positions/${positionId}`,
    );
    return parsePosition(raw);
  }

  async closePosition(
    positionId: string,
    params?: {
      quantity?: number;
      limit_price?: number;
    },
  ): Promise<Position> {
    const raw = await this.request<Position>(
      `/api/v1/positions/${positionId}/close`,
      {
        method: "POST",
        body: JSON.stringify(params || {}),
      },
    );
    return parsePosition(raw);
  }

  // Orders
  async placeOrder(params: PlaceOrderRequest): Promise<Order> {
    return this.request<Order>("/api/v1/orders", {
      method: "POST",
      body: JSON.stringify(params),
    });
  }

  async getOrder(orderId: string): Promise<Order> {
    return this.request<Order>(`/api/v1/orders/${orderId}`);
  }

  async cancelOrder(orderId: string): Promise<Order> {
    return this.request<Order>(`/api/v1/orders/${orderId}/cancel`, {
      method: "POST",
    });
  }

  // Backtest
  async runBacktest(params: BacktestParams): Promise<BacktestResult> {
    return this.request<BacktestResult>("/api/v1/backtest", {
      method: "POST",
      body: JSON.stringify(params),
    });
  }

  async getBacktestResults(params?: {
    strategy_type?: string;
    status?: string;
    limit?: number;
    offset?: number;
  }): Promise<BacktestResult[]> {
    const searchParams = new URLSearchParams();
    if (params?.strategy_type)
      searchParams.set("strategy_type", params.strategy_type);
    if (params?.status) searchParams.set("status", params.status);
    if (params?.limit) searchParams.set("limit", String(params.limit));
    if (params?.offset) searchParams.set("offset", String(params.offset));
    const query = searchParams.toString();
    return this.request<BacktestResult[]>(
      `/api/v1/backtest/results${query ? `?${query}` : ""}`,
    );
  }

  async getBacktestResult(resultId: string): Promise<BacktestResult> {
    return this.request<BacktestResult>(`/api/v1/backtest/results/${resultId}`);
  }

  async getMarketRegime(): Promise<MarketRegimeResponse> {
    return this.request<MarketRegimeResponse>("/api/v1/regime/current");
  }

  // Vault (Connected Wallets for Live Trading)
  async getConnectedWallets(): Promise<ConnectedWallet[]> {
    return this.request<ConnectedWallet[]>("/api/v1/vault/wallets");
  }

  async getConnectedWallet(address: string): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>(`/api/v1/vault/wallets/${address}`);
  }

  async connectWallet(params: StoreWalletRequest): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>("/api/v1/vault/wallets", {
      method: "POST",
      body: JSON.stringify(params),
    });
  }

  async disconnectWallet(address: string): Promise<void> {
    return this.request<void>(`/api/v1/vault/wallets/${address}`, {
      method: "DELETE",
    });
  }

  async setPrimaryWallet(address: string): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>(
      `/api/v1/vault/wallets/${address}/primary`,
      {
        method: "PUT",
      },
    );
  }

  async getWalletBalance(
    address: string,
  ): Promise<{ address: string; usdc_balance: number }> {
    return this.request<{ address: string; usdc_balance: number }>(
      `/api/v1/vault/wallets/${address}/balance`,
    );
  }

  // User Management (Admin only)
  async listUsers(): Promise<UserListItem[]> {
    return this.request<UserListItem[]>("/api/v1/users");
  }

  async createUser(params: CreateUserRequest): Promise<UserListItem> {
    return this.request<UserListItem>("/api/v1/users", {
      method: "POST",
      body: JSON.stringify(params),
    });
  }

  async getUser(userId: string): Promise<UserListItem> {
    return this.request<UserListItem>(`/api/v1/users/${userId}`);
  }

  async updateUser(
    userId: string,
    params: UpdateUserRequest,
  ): Promise<UserListItem> {
    return this.request<UserListItem>(`/api/v1/users/${userId}`, {
      method: "PATCH",
      body: JSON.stringify(params),
    });
  }

  async deleteUser(userId: string): Promise<void> {
    return this.request<void>(`/api/v1/users/${userId}`, {
      method: "DELETE",
    });
  }

  // Admin Workspace Management (Platform Admin only)
  async adminListWorkspaces(): Promise<WorkspaceListItem[]> {
    return this.request<WorkspaceListItem[]>("/api/v1/admin/workspaces");
  }

  async adminCreateWorkspace(
    params: CreateWorkspaceRequest,
  ): Promise<Workspace> {
    return this.request<Workspace>("/api/v1/admin/workspaces", {
      method: "POST",
      body: JSON.stringify(params),
    });
  }

  async adminGetWorkspace(workspaceId: string): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/admin/workspaces/${workspaceId}`);
  }

  async adminUpdateWorkspace(
    workspaceId: string,
    params: UpdateWorkspaceRequest,
  ): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/admin/workspaces/${workspaceId}`, {
      method: "PUT",
      body: JSON.stringify(params),
    });
  }

  async adminDeleteWorkspace(workspaceId: string): Promise<void> {
    return this.request<void>(`/api/v1/admin/workspaces/${workspaceId}`, {
      method: "DELETE",
    });
  }

  // User Workspaces
  async listWorkspaces(): Promise<WorkspaceListItem[]> {
    return this.request<WorkspaceListItem[]>("/api/v1/workspaces");
  }

  async getCurrentWorkspace(): Promise<Workspace> {
    return this.request<Workspace>("/api/v1/workspaces/current");
  }

  async getWorkspace(workspaceId: string): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/workspaces/${workspaceId}`);
  }

  async updateWorkspace(
    workspaceId: string,
    params: UpdateWorkspaceRequest,
  ): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/workspaces/${workspaceId}`, {
      method: "PUT",
      body: JSON.stringify(params),
    });
  }

  async switchWorkspace(workspaceId: string): Promise<void> {
    return this.request<void>(`/api/v1/workspaces/${workspaceId}/switch`, {
      method: "POST",
    });
  }

  async listWorkspaceMembers(workspaceId: string): Promise<WorkspaceMember[]> {
    return this.request<WorkspaceMember[]>(
      `/api/v1/workspaces/${workspaceId}/members`,
    );
  }

  async updateMemberRole(
    workspaceId: string,
    memberId: string,
    role: WorkspaceRole,
  ): Promise<WorkspaceMember> {
    return this.request<WorkspaceMember>(
      `/api/v1/workspaces/${workspaceId}/members/${memberId}`,
      {
        method: "PUT",
        body: JSON.stringify({ role }),
      },
    );
  }

  async removeMember(workspaceId: string, memberId: string): Promise<void> {
    return this.request<void>(
      `/api/v1/workspaces/${workspaceId}/members/${memberId}`,
      {
        method: "DELETE",
      },
    );
  }

  async getServiceStatus(workspaceId: string): Promise<ServiceStatus> {
    return this.request<ServiceStatus>(
      `/api/v1/workspaces/${workspaceId}/service-status`,
    );
  }

  async getDynamicTunerStatus(workspaceId: string): Promise<DynamicTunerStatus> {
    return this.request<DynamicTunerStatus>(
      `/api/v1/workspaces/${workspaceId}/dynamic-tuning/status`,
    );
  }

  async updateOpportunitySelection(
    workspaceId: string,
    params: UpdateOpportunitySelectionRequest,
  ): Promise<DynamicTunerStatus["opportunity_selection"]> {
    return this.request<DynamicTunerStatus["opportunity_selection"]>(
      `/api/v1/workspaces/${workspaceId}/dynamic-tuning/opportunity-selection`,
      {
        method: "PUT",
        body: JSON.stringify(params),
      },
    );
  }

  async updateArbExecutorConfig(
    workspaceId: string,
    params: {
      position_size?: number;
      min_net_profit?: number;
      min_book_depth?: number;
      max_signal_age_secs?: number;
    },
  ): Promise<{
    position_size: number | null;
    min_net_profit: number | null;
    min_book_depth: number | null;
    max_signal_age_secs: number | null;
  }> {
    return this.request(
      `/api/v1/workspaces/${workspaceId}/dynamic-tuning/arb-executor`,
      { method: "PUT", body: JSON.stringify(params) },
    );
  }

  async getDynamicTuningHistory(
    workspaceId: string,
    params?: {
      limit?: number;
      offset?: number;
    },
  ): Promise<DynamicConfigHistoryEntry[]> {
    const searchParams = new URLSearchParams();
    if (params?.limit) searchParams.set("limit", String(params.limit));
    if (params?.offset) searchParams.set("offset", String(params.offset));
    const query = searchParams.toString();
    return this.request<DynamicConfigHistoryEntry[]>(
      `/api/v1/workspaces/${workspaceId}/dynamic-tuning/history${query ? `?${query}` : ""}`,
    );
  }

  // Workspace Invites
  async listWorkspaceInvites(workspaceId: string): Promise<WorkspaceInvite[]> {
    return this.request<WorkspaceInvite[]>(
      `/api/v1/workspaces/${workspaceId}/invites`,
    );
  }

  async createInvite(
    workspaceId: string,
    params: CreateInviteRequest,
  ): Promise<WorkspaceInvite> {
    return this.request<WorkspaceInvite>(
      `/api/v1/workspaces/${workspaceId}/invites`,
      {
        method: "POST",
        body: JSON.stringify(params),
      },
    );
  }

  async revokeInvite(workspaceId: string, inviteId: string): Promise<void> {
    return this.request<void>(
      `/api/v1/workspaces/${workspaceId}/invites/${inviteId}`,
      {
        method: "DELETE",
      },
    );
  }

  async getInviteInfo(token: string): Promise<InviteInfo> {
    return this.request<InviteInfo>(`/api/v1/invites/${token}`);
  }

  async acceptInvite(
    token: string,
    params?: AcceptInviteRequest,
    options?: {
      auth?: boolean;
    },
  ): Promise<AcceptInviteResponse> {
    return this.request<AcceptInviteResponse>(
      `/api/v1/invites/${token}/accept`,
      {
        method: "POST",
        body: JSON.stringify(params || {}),
      },
      options,
    );
  }

  // Risk Monitoring
  async getRiskStatus(workspaceId: string): Promise<RiskStatus> {
    return this.request<RiskStatus>(
      `/api/v1/workspaces/${workspaceId}/risk/status`,
    );
  }

  async manualTripCircuitBreaker(
    workspaceId: string,
  ): Promise<CircuitBreakerStatus> {
    return this.request<CircuitBreakerStatus>(
      `/api/v1/workspaces/${workspaceId}/risk/circuit-breaker/trip`,
      { method: "POST" },
    );
  }

  async resetCircuitBreaker(
    workspaceId: string,
  ): Promise<CircuitBreakerStatus> {
    return this.request<CircuitBreakerStatus>(
      `/api/v1/workspaces/${workspaceId}/risk/circuit-breaker/reset`,
      { method: "POST" },
    );
  }

  async updateCircuitBreakerConfig(
    workspaceId: string,
    params: {
      max_daily_loss?: number;
      max_drawdown_pct?: number;
      max_consecutive_losses?: number;
      cooldown_minutes?: number;
      enabled?: boolean;
    },
  ): Promise<CircuitBreakerConfig> {
    return this.request<CircuitBreakerConfig>(
      `/api/v1/workspaces/${workspaceId}/risk/circuit-breaker/config`,
      { method: "PUT", body: JSON.stringify(params) },
    );
  }

  // Activity Feed
  async getActivity(params?: {
    limit?: number;
    offset?: number;
  }): Promise<Activity[]> {
    const searchParams = new URLSearchParams();
    if (params?.limit) searchParams.set("limit", String(params.limit));
    if (params?.offset) searchParams.set("offset", String(params.offset));
    const query = searchParams.toString();
    return this.request<Activity[]>(
      `/api/v1/activity${query ? `?${query}` : ""}`,
    );
  }

  // Quant Signals
  async getFlowFeatures(params: {
    condition_id: string;
    window_minutes?: number;
  }): Promise<FlowFeatureResponse[]> {
    const sp = new URLSearchParams();
    sp.set("condition_id", params.condition_id);
    if (params.window_minutes) sp.set("window_minutes", String(params.window_minutes));
    const raw = await this.request<FlowFeatureResponse[]>(`/api/v1/signals/flow?${sp}`);
    return raw.map(parseFlowFeature);
  }

  async getRecentSignals(params?: {
    kind?: string;
    limit?: number;
  }): Promise<RecentSignalResponse[]> {
    const sp = new URLSearchParams();
    if (params?.kind) sp.set("kind", params.kind);
    if (params?.limit) sp.set("limit", String(params.limit));
    const q = sp.toString();
    const raw = await this.request<RecentSignalResponse[]>(
      `/api/v1/signals/recent${q ? `?${q}` : ""}`,
    );
    return raw.map((r) => ({
      ...r,
      size_usd: r.size_usd != null ? Number(r.size_usd) : null,
    }));
  }

  async getStrategyPerformance(params?: {
    period_days?: number;
  }): Promise<StrategyPerformanceResponse[]> {
    const sp = new URLSearchParams();
    if (params?.period_days) sp.set("period_days", String(params.period_days));
    const q = sp.toString();
    const raw = await this.request<StrategyPerformanceResponse[]>(
      `/api/v1/signals/performance${q ? `?${q}` : ""}`,
    );
    return raw.map(parsePerformance);
  }

  async getMarketMetadata(params?: {
    category?: string;
    active?: boolean;
    limit?: number;
  }): Promise<MarketMetadataResponse[]> {
    const sp = new URLSearchParams();
    if (params?.category) sp.set("category", params.category);
    if (params?.active !== undefined) sp.set("active", String(params.active));
    if (params?.limit) sp.set("limit", String(params.limit));
    const q = sp.toString();
    const raw = await this.request<MarketMetadataResponse[]>(
      `/api/v1/signals/metadata${q ? `?${q}` : ""}`,
    );
    return raw.map((r) => ({
      ...r,
      volume: Number(r.volume),
      liquidity: Number(r.liquidity),
    }));
  }
}

export const api = new ApiClient(API_BASE_URL);
export default api;
