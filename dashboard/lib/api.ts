import type {
  Position,
  PositionStatus,
  Market,
  Orderbook,
  TrackedWallet,
  WalletMetrics,
  Order,
  PlaceOrderRequest,
  BacktestParams,
  BacktestResult,
  HealthResponse,
  ApiError,
  LiveTrade,
  DiscoveredWallet,
  DemoPnlSimulation,
  User,
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
  WorkspaceAllocation,
  RotationHistoryEntry,
  OnboardingStatus,
  CreateWorkspaceRequest,
  UpdateWorkspaceRequest,
  CreateInviteRequest,
  InviteInfo,
  AcceptInviteRequest,
  AcceptInviteResponse,
  AddAllocationRequest,
  UpdateAllocationRequest,
  SetBudgetRequest,
  AutoSetupConfig,
  SetupMode,
  WorkspaceRole,
  OptimizerStatus,
  DemoPosition,
  CreateDemoPositionRequest,
  UpdateDemoPositionRequest,
  DemoBalance,
} from '@/types/api';

const API_BASE_URL = process.env.NEXT_PUBLIC_API_URL || 'http://localhost:3001';

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
      method: 'POST',
      body: JSON.stringify(body),
    });
  }

  async put<T>(endpoint: string, body: unknown): Promise<T> {
    return this.request<T>(endpoint, {
      method: 'PUT',
      body: JSON.stringify(body),
    });
  }

  async delete<T>(endpoint: string): Promise<T> {
    return this.request<T>(endpoint, {
      method: 'DELETE',
    });
  }

  private async request<T>(
    endpoint: string,
    options: RequestInit = {}
  ): Promise<T> {
    const headers: HeadersInit = {
      'Content-Type': 'application/json',
      ...options.headers,
    };

    if (this.token) {
      (headers as Record<string, string>)['Authorization'] = `Bearer ${this.token}`;
    }

    const response = await fetch(`${this.baseUrl}${endpoint}`, {
      ...options,
      headers,
    });

    if (!response.ok) {
      const error: ApiError = await response.json().catch(() => ({
        code: 'UNKNOWN_ERROR',
        message: `HTTP ${response.status}`,
      }));
      throw new Error(error.message || `HTTP ${response.status}`);
    }

    // Handle 204 No Content
    if (response.status === 204) {
      return undefined as T;
    }

    return response.json();
  }

  // Health Check
  async healthCheck(): Promise<HealthResponse> {
    return this.request<HealthResponse>('/health');
  }

  async readyCheck(): Promise<HealthResponse> {
    return this.request<HealthResponse>('/ready');
  }

  // Auth
  async login(email: string, password: string): Promise<AuthResponse> {
    const response = await this.request<AuthResponse>('/api/v1/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email, password }),
    });
    this.setToken(response.token);
    return response;
  }

  async refreshToken(): Promise<AuthResponse> {
    const response = await this.request<AuthResponse>('/api/v1/auth/refresh', {
      method: 'POST',
    });
    this.setToken(response.token);
    return response;
  }

  async getCurrentUser(): Promise<User> {
    return this.request<User>('/api/v1/auth/me');
  }

  async forgotPassword(email: string): Promise<{ message: string }> {
    return this.request<{ message: string }>('/api/v1/auth/forgot-password', {
      method: 'POST',
      body: JSON.stringify({ email }),
    });
  }

  async resetPassword(token: string, password: string): Promise<{ message: string }> {
    return this.request<{ message: string }>('/api/v1/auth/reset-password', {
      method: 'POST',
      body: JSON.stringify({ token, password }),
    });
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
    if (params?.category) searchParams.set('category', params.category);
    if (params?.active !== undefined) searchParams.set('active', String(params.active));
    if (params?.min_volume) searchParams.set('min_volume', String(params.min_volume));
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.offset) searchParams.set('offset', String(params.offset));
    const query = searchParams.toString();
    return this.request<Market[]>(`/api/v1/markets${query ? `?${query}` : ''}`);
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
    outcome?: 'yes' | 'no';
    copy_trades_only?: boolean;
    status?: PositionStatus;
    limit?: number;
    offset?: number;
  }): Promise<Position[]> {
    const searchParams = new URLSearchParams();
    if (params?.market_id) searchParams.set('market_id', params.market_id);
    if (params?.outcome) searchParams.set('outcome', params.outcome);
    if (params?.copy_trades_only !== undefined) searchParams.set('copy_trades_only', String(params.copy_trades_only));
    if (params?.status) searchParams.set('status', params.status);
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.offset) searchParams.set('offset', String(params.offset));
    const query = searchParams.toString();
    return this.request<Position[]>(`/api/v1/positions${query ? `?${query}` : ''}`);
  }

  async getPosition(positionId: string): Promise<Position> {
    return this.request<Position>(`/api/v1/positions/${positionId}`);
  }

  async closePosition(positionId: string, params?: {
    quantity?: number;
    limit_price?: number;
  }): Promise<Position> {
    return this.request<Position>(`/api/v1/positions/${positionId}/close`, {
      method: 'POST',
      body: JSON.stringify(params || {}),
    });
  }

  // Wallets
  async getWallets(params?: {
    copy_enabled?: boolean;
    min_score?: number;
    limit?: number;
    offset?: number;
  }): Promise<TrackedWallet[]> {
    const searchParams = new URLSearchParams();
    if (params?.copy_enabled !== undefined) searchParams.set('copy_enabled', String(params.copy_enabled));
    if (params?.min_score) searchParams.set('min_score', String(params.min_score));
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.offset) searchParams.set('offset', String(params.offset));
    const query = searchParams.toString();
    return this.request<TrackedWallet[]>(`/api/v1/wallets${query ? `?${query}` : ''}`);
  }

  async getWallet(address: string): Promise<TrackedWallet> {
    return this.request<TrackedWallet>(`/api/v1/wallets/${address}`);
  }

  async addWallet(params: {
    address: string;
    label?: string;
    copy_enabled?: boolean;
    allocation_pct?: number;
    max_position_size?: number;
  }): Promise<TrackedWallet> {
    return this.request<TrackedWallet>('/api/v1/wallets', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async updateWallet(address: string, params: {
    label?: string;
    copy_enabled?: boolean;
    allocation_pct?: number;
    max_position_size?: number;
  }): Promise<TrackedWallet> {
    return this.request<TrackedWallet>(`/api/v1/wallets/${address}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async deleteWallet(address: string): Promise<void> {
    return this.request<void>(`/api/v1/wallets/${address}`, {
      method: 'DELETE',
    });
  }

  async getWalletMetrics(address: string): Promise<WalletMetrics> {
    return this.request<WalletMetrics>(`/api/v1/wallets/${address}/metrics`);
  }

  // Orders
  async placeOrder(params: PlaceOrderRequest): Promise<Order> {
    return this.request<Order>('/api/v1/orders', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async getOrder(orderId: string): Promise<Order> {
    return this.request<Order>(`/api/v1/orders/${orderId}`);
  }

  async cancelOrder(orderId: string): Promise<Order> {
    return this.request<Order>(`/api/v1/orders/${orderId}/cancel`, {
      method: 'POST',
    });
  }

  // Backtest
  async runBacktest(params: BacktestParams): Promise<BacktestResult> {
    return this.request<BacktestResult>('/api/v1/backtest', {
      method: 'POST',
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
    if (params?.strategy_type) searchParams.set('strategy_type', params.strategy_type);
    if (params?.status) searchParams.set('status', params.status);
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.offset) searchParams.set('offset', String(params.offset));
    const query = searchParams.toString();
    return this.request<BacktestResult[]>(`/api/v1/backtest/results${query ? `?${query}` : ''}`);
  }

  async getBacktestResult(resultId: string): Promise<BacktestResult> {
    return this.request<BacktestResult>(`/api/v1/backtest/results/${resultId}`);
  }

  // Discovery
  async getLiveTrades(params?: {
    wallet?: string;
    limit?: number;
    min_value?: number;
  }): Promise<LiveTrade[]> {
    const searchParams = new URLSearchParams();
    if (params?.wallet) searchParams.set('wallet', params.wallet);
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.min_value) searchParams.set('min_value', String(params.min_value));
    const query = searchParams.toString();
    return this.request<LiveTrade[]>(`/api/v1/discover/trades${query ? `?${query}` : ''}`);
  }

  async discoverWallets(params?: {
    sort_by?: 'roi' | 'sharpe' | 'winRate' | 'trades';
    period?: '7d' | '30d' | '90d';
    min_trades?: number;
    min_win_rate?: number;
    limit?: number;
  }): Promise<DiscoveredWallet[]> {
    const searchParams = new URLSearchParams();
    if (params?.sort_by) searchParams.set('sort_by', params.sort_by);
    if (params?.period) searchParams.set('period', params.period);
    if (params?.min_trades) searchParams.set('min_trades', String(params.min_trades));
    if (params?.min_win_rate) searchParams.set('min_win_rate', String(params.min_win_rate));
    if (params?.limit) searchParams.set('limit', String(params.limit));
    const query = searchParams.toString();
    return this.request<DiscoveredWallet[]>(`/api/v1/discover/wallets${query ? `?${query}` : ''}`);
  }

  async simulateDemoPnl(params?: {
    amount?: number;
    period?: '7d' | '30d' | '90d';
    wallets?: string;
  }): Promise<DemoPnlSimulation> {
    const searchParams = new URLSearchParams();
    if (params?.amount) searchParams.set('amount', String(params.amount));
    if (params?.period) searchParams.set('period', params.period);
    if (params?.wallets) searchParams.set('wallets', params.wallets);
    const query = searchParams.toString();
    return this.request<DemoPnlSimulation>(`/api/v1/discover/simulate${query ? `?${query}` : ''}`);
  }

  // Vault (Connected Wallets for Live Trading)
  async getConnectedWallets(): Promise<ConnectedWallet[]> {
    return this.request<ConnectedWallet[]>('/api/v1/vault/wallets');
  }

  async getConnectedWallet(address: string): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>(`/api/v1/vault/wallets/${address}`);
  }

  async connectWallet(params: StoreWalletRequest): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>('/api/v1/vault/wallets', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async disconnectWallet(address: string): Promise<void> {
    return this.request<void>(`/api/v1/vault/wallets/${address}`, {
      method: 'DELETE',
    });
  }

  async setPrimaryWallet(address: string): Promise<ConnectedWallet> {
    return this.request<ConnectedWallet>(`/api/v1/vault/wallets/${address}/primary`, {
      method: 'PUT',
    });
  }

  // User Management (Admin only)
  async listUsers(): Promise<UserListItem[]> {
    return this.request<UserListItem[]>('/api/v1/users');
  }

  async createUser(params: CreateUserRequest): Promise<UserListItem> {
    return this.request<UserListItem>('/api/v1/users', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async getUser(userId: string): Promise<UserListItem> {
    return this.request<UserListItem>(`/api/v1/users/${userId}`);
  }

  async updateUser(userId: string, params: UpdateUserRequest): Promise<UserListItem> {
    return this.request<UserListItem>(`/api/v1/users/${userId}`, {
      method: 'PATCH',
      body: JSON.stringify(params),
    });
  }

  async deleteUser(userId: string): Promise<void> {
    return this.request<void>(`/api/v1/users/${userId}`, {
      method: 'DELETE',
    });
  }

  // Admin Workspace Management (Platform Admin only)
  async adminListWorkspaces(): Promise<WorkspaceListItem[]> {
    return this.request<WorkspaceListItem[]>('/api/v1/admin/workspaces');
  }

  async adminCreateWorkspace(params: CreateWorkspaceRequest): Promise<Workspace> {
    return this.request<Workspace>('/api/v1/admin/workspaces', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async adminGetWorkspace(workspaceId: string): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/admin/workspaces/${workspaceId}`);
  }

  async adminUpdateWorkspace(workspaceId: string, params: UpdateWorkspaceRequest): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/admin/workspaces/${workspaceId}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async adminDeleteWorkspace(workspaceId: string): Promise<void> {
    return this.request<void>(`/api/v1/admin/workspaces/${workspaceId}`, {
      method: 'DELETE',
    });
  }

  // User Workspaces
  async listWorkspaces(): Promise<WorkspaceListItem[]> {
    return this.request<WorkspaceListItem[]>('/api/v1/workspaces');
  }

  async getCurrentWorkspace(): Promise<Workspace> {
    return this.request<Workspace>('/api/v1/workspaces/current');
  }

  async getWorkspace(workspaceId: string): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/workspaces/${workspaceId}`);
  }

  async updateWorkspace(workspaceId: string, params: UpdateWorkspaceRequest): Promise<Workspace> {
    return this.request<Workspace>(`/api/v1/workspaces/${workspaceId}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async switchWorkspace(workspaceId: string): Promise<void> {
    return this.request<void>(`/api/v1/workspaces/${workspaceId}/switch`, {
      method: 'POST',
    });
  }

  async listWorkspaceMembers(workspaceId: string): Promise<WorkspaceMember[]> {
    return this.request<WorkspaceMember[]>(`/api/v1/workspaces/${workspaceId}/members`);
  }

  async updateMemberRole(workspaceId: string, memberId: string, role: WorkspaceRole): Promise<WorkspaceMember> {
    return this.request<WorkspaceMember>(`/api/v1/workspaces/${workspaceId}/members/${memberId}`, {
      method: 'PUT',
      body: JSON.stringify({ role }),
    });
  }

  async removeMember(workspaceId: string, memberId: string): Promise<void> {
    return this.request<void>(`/api/v1/workspaces/${workspaceId}/members/${memberId}`, {
      method: 'DELETE',
    });
  }

  async getOptimizerStatus(workspaceId: string): Promise<OptimizerStatus> {
    return this.request<OptimizerStatus>(`/api/v1/workspaces/${workspaceId}/optimizer-status`);
  }

  // Workspace Invites
  async listWorkspaceInvites(workspaceId: string): Promise<WorkspaceInvite[]> {
    return this.request<WorkspaceInvite[]>(`/api/v1/workspaces/${workspaceId}/invites`);
  }

  async createInvite(workspaceId: string, params: CreateInviteRequest): Promise<WorkspaceInvite> {
    return this.request<WorkspaceInvite>(`/api/v1/workspaces/${workspaceId}/invites`, {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async revokeInvite(workspaceId: string, inviteId: string): Promise<void> {
    return this.request<void>(`/api/v1/workspaces/${workspaceId}/invites/${inviteId}`, {
      method: 'DELETE',
    });
  }

  async getInviteInfo(token: string): Promise<InviteInfo> {
    return this.request<InviteInfo>(`/api/v1/invites/${token}`);
  }

  async acceptInvite(token: string, params?: AcceptInviteRequest): Promise<AcceptInviteResponse> {
    return this.request<AcceptInviteResponse>(`/api/v1/invites/${token}/accept`, {
      method: 'POST',
      body: JSON.stringify(params || {}),
    });
  }

  // Workspace Allocations
  async listAllocations(): Promise<WorkspaceAllocation[]> {
    return this.request<WorkspaceAllocation[]>('/api/v1/allocations');
  }

  async addAllocation(address: string, params?: AddAllocationRequest): Promise<WorkspaceAllocation> {
    return this.request<WorkspaceAllocation>(`/api/v1/allocations/${address}`, {
      method: 'POST',
      body: JSON.stringify(params || {}),
    });
  }

  async updateAllocation(address: string, params: UpdateAllocationRequest): Promise<WorkspaceAllocation> {
    return this.request<WorkspaceAllocation>(`/api/v1/allocations/${address}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async removeAllocation(address: string): Promise<void> {
    return this.request<void>(`/api/v1/allocations/${address}`, {
      method: 'DELETE',
    });
  }

  async promoteAllocation(address: string): Promise<WorkspaceAllocation> {
    return this.request<WorkspaceAllocation>(`/api/v1/allocations/${address}/promote`, {
      method: 'POST',
    });
  }

  async demoteAllocation(address: string): Promise<WorkspaceAllocation> {
    return this.request<WorkspaceAllocation>(`/api/v1/allocations/${address}/demote`, {
      method: 'POST',
    });
  }

  // Auto-Rotation
  async listRotationHistory(params?: {
    limit?: number;
    offset?: number;
    unacknowledged_only?: boolean;
  }): Promise<RotationHistoryEntry[]> {
    const searchParams = new URLSearchParams();
    if (params?.limit) searchParams.set('limit', String(params.limit));
    if (params?.offset) searchParams.set('offset', String(params.offset));
    if (params?.unacknowledged_only !== undefined) {
      searchParams.set('unacknowledged_only', String(params.unacknowledged_only));
    }
    const query = searchParams.toString();
    return this.request<RotationHistoryEntry[]>(`/api/v1/auto-rotation/history${query ? `?${query}` : ''}`);
  }

  async acknowledgeRotation(entryId: string): Promise<RotationHistoryEntry> {
    return this.request<RotationHistoryEntry>(`/api/v1/auto-rotation/${entryId}/acknowledge`, {
      method: 'PUT',
    });
  }

  async triggerOptimization(): Promise<void> {
    return this.request<void>('/api/v1/auto-rotation/trigger', {
      method: 'POST',
    });
  }

  // Onboarding
  async getOnboardingStatus(): Promise<OnboardingStatus> {
    return this.request<OnboardingStatus>('/api/v1/onboarding/status');
  }

  async setOnboardingMode(mode: SetupMode): Promise<OnboardingStatus> {
    return this.request<OnboardingStatus>('/api/v1/onboarding/mode', {
      method: 'PUT',
      body: JSON.stringify({ mode }),
    });
  }

  async setOnboardingBudget(params: SetBudgetRequest): Promise<OnboardingStatus> {
    return this.request<OnboardingStatus>('/api/v1/onboarding/budget', {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async runAutoSetup(config?: AutoSetupConfig): Promise<{ selected_wallets: string[]; message: string }> {
    return this.request<{ selected_wallets: string[]; message: string }>('/api/v1/onboarding/auto-setup', {
      method: 'POST',
      body: JSON.stringify(config || {}),
    });
  }

  async completeOnboarding(): Promise<OnboardingStatus> {
    return this.request<OnboardingStatus>('/api/v1/onboarding/complete', {
      method: 'PUT',
    });
  }

  // Demo Positions
  async listDemoPositions(params?: {
    status?: 'open' | 'closed' | 'all';
  }): Promise<DemoPosition[]> {
    const searchParams = new URLSearchParams();
    if (params?.status) searchParams.set('status', params.status);
    const query = searchParams.toString();
    return this.request<DemoPosition[]>(`/api/v1/demo/positions${query ? `?${query}` : ''}`);
  }

  async createDemoPosition(params: CreateDemoPositionRequest): Promise<DemoPosition> {
    return this.request<DemoPosition>('/api/v1/demo/positions', {
      method: 'POST',
      body: JSON.stringify(params),
    });
  }

  async updateDemoPosition(positionId: string, params: UpdateDemoPositionRequest): Promise<DemoPosition> {
    return this.request<DemoPosition>(`/api/v1/demo/positions/${positionId}`, {
      method: 'PUT',
      body: JSON.stringify(params),
    });
  }

  async deleteDemoPosition(positionId: string): Promise<void> {
    return this.request<void>(`/api/v1/demo/positions/${positionId}`, {
      method: 'DELETE',
    });
  }

  async getDemoBalance(): Promise<DemoBalance> {
    return this.request<DemoBalance>('/api/v1/demo/balance');
  }

  async updateDemoBalance(balance: number): Promise<DemoBalance> {
    return this.request<DemoBalance>('/api/v1/demo/balance', {
      method: 'PUT',
      body: JSON.stringify({ balance }),
    });
  }

  async resetDemoPortfolio(): Promise<DemoBalance> {
    return this.request<DemoBalance>('/api/v1/demo/reset', {
      method: 'POST',
    });
  }
}

export const api = new ApiClient(API_BASE_URL);
export default api;
