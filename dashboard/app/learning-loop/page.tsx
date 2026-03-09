"use client";

import { useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { InfoTooltip } from "@/components/shared/InfoTooltip";
import { LiveIndicator } from "@/components/shared/LiveIndicator";
import { MetricCard } from "@/components/shared/MetricCard";
import { PageIntro } from "@/components/shared/PageIntro";
import {
  useCreateLearningModelMutation,
  useCreateLearningRolloutMutation,
  useLearningOverviewQuery,
  useLearningModelStatusMutation,
  useLearningRolloutDetailQuery,
  useUpdateLearningModelMutation,
  useRolloutStatusActionMutation,
  useUpdateLearningRolloutMutation,
} from "@/hooks/queries/useTradeFlowQuery";
import { cn } from "@/lib/utils";
import { useAuthStore } from "@/stores/auth-store";
import { useToastStore } from "@/stores/toast-store";
import type { LearningModelSummary, LearningRolloutStatus } from "@/types/api";
import {
  BrainCircuit,
  FlaskConical,
  PencilLine,
  Play,
  ShieldCheck,
  Square,
  Pause,
  Cpu,
} from "lucide-react";

type WindowKey = "7d" | "30d" | "90d";
type RolloutMode = "shadow" | "canary" | "bounded" | "full";
type AuthorityLevel =
  | "observe"
  | "tail_reject"
  | "size_adjust"
  | "priority_only"
  | "full";
type ModelStrategyScope = "arb" | "quant";
type ModelType =
  | "heuristic_shadow_baseline"
  | "trained_linear_probability_v1"
  | "trained_linear_regression_v1";
type ModelStatus =
  | "draft"
  | "shadow"
  | "canary"
  | "active"
  | "retired"
  | "disabled";

const WINDOWS: Record<WindowKey, { label: string; days: number }> = {
  "7d": { label: "7 Days", days: 7 },
  "30d": { label: "30 Days", days: 30 },
  "90d": { label: "90 Days", days: 90 },
};

const ROLLOUT_MODES: RolloutMode[] = ["shadow", "canary", "bounded", "full"];
const AUTHORITY_OPTIONS: AuthorityLevel[] = [
  "observe",
  "tail_reject",
  "size_adjust",
  "priority_only",
  "full",
];
const MODEL_TYPES: ModelType[] = [
  "trained_linear_probability_v1",
  "trained_linear_regression_v1",
  "heuristic_shadow_baseline",
];
const MODEL_STATUSES: ModelStatus[] = [
  "draft",
  "shadow",
  "canary",
  "active",
  "retired",
  "disabled",
];

function formatRate(numerator: number, denominator: number) {
  if (denominator <= 0) return "—";
  return `${((numerator / denominator) * 100).toFixed(1)}%`;
}

function formatPct(value?: number | null) {
  if (value == null || Number.isNaN(value)) return "—";
  return `${(value * 100).toFixed(1)}%`;
}

function formatMs(value?: number | null) {
  if (value == null || Number.isNaN(value)) return "—";
  return `${value.toFixed(value >= 100 ? 0 : 1)} ms`;
}

function summarizeObject(value: Record<string, unknown>) {
  const entries = Object.entries(value).slice(0, 3);
  if (entries.length === 0) return "No metrics yet";

  return entries
    .map(([key, raw]) => {
      if (typeof raw === "number") {
        return `${key}: ${Number.isInteger(raw) ? raw : raw.toFixed(3)}`;
      }
      if (typeof raw === "string" || typeof raw === "boolean") {
        return `${key}: ${String(raw)}`;
      }
      return `${key}: …`;
    })
    .join(" • ");
}

function statusBadgeClass(status: string) {
  switch (status) {
    case "active":
    case "ok":
      return "border-green-500/20 bg-green-500/10 text-green-600";
    case "canary":
    case "shadow":
    case "warn":
    case "paused":
      return "border-amber-500/20 bg-amber-500/10 text-amber-600";
    case "rolled_back":
    case "rollback":
    case "disabled":
    case "completed":
      return "border-red-500/20 bg-red-500/10 text-red-600";
    default:
      return "border-border/60 bg-muted text-muted-foreground";
  }
}

function asNumber(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? String(value) : "";
}

function parseNumberInput(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function rolloutDefaultAuthority(mode: RolloutMode): AuthorityLevel {
  return mode === "shadow" ? "observe" : "tail_reject";
}

function buildBounds(
  threshold: string,
  maxSizeReductionPct: string,
): Record<string, number> | undefined {
  const bounds: Record<string, number> = {};
  const parsedThreshold = parseNumberInput(threshold);
  const parsedMaxSizeReductionPct = parseNumberInput(maxSizeReductionPct);
  if (parsedThreshold != null) bounds.threshold = parsedThreshold;
  if (parsedMaxSizeReductionPct != null) {
    bounds.max_size_reduction_pct = parsedMaxSizeReductionPct;
  }
  return Object.keys(bounds).length > 0 ? bounds : undefined;
}

function buildGuardrails(
  maxFailureRate: string,
  maxOneLeggedRate: string,
  maxDrawdownPct: string,
  maxLatencyP90Ms: string,
  minEdgeCaptureRatio: string,
): Record<string, number> | undefined {
  const guardrails: Record<string, number> = {};
  const pairs: Array<[string, string]> = [
    ["max_failure_rate", maxFailureRate],
    ["max_one_legged_rate", maxOneLeggedRate],
    ["max_drawdown_pct", maxDrawdownPct],
    ["max_latency_p90_ms", maxLatencyP90Ms],
    ["min_edge_capture_ratio", minEdgeCaptureRatio],
  ];
  for (const [key, raw] of pairs) {
    const parsed = parseNumberInput(raw);
    if (parsed != null) {
      guardrails[key] = parsed;
    }
  }
  return Object.keys(guardrails).length > 0 ? guardrails : undefined;
}

function featureViewForScope(strategyScope: ModelStrategyScope) {
  return strategyScope === "arb"
    ? "canonical_arb_learning_attempts"
    : "canonical_quant_learning_decisions";
}

function targetOptionsForScope(strategyScope: ModelStrategyScope) {
  return strategyScope === "arb"
    ? ["open_success_probability", "one_legged_risk", "realized_edge_capture"]
    : ["execute_success_probability", "realized_pnl_sign", "realized_edge_capture"];
}

function ModelActionButtons({
  model,
  selected,
  onSelect,
}: {
  model: LearningModelSummary;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  const toast = useToastStore();
  const activateMutation = useLearningModelStatusMutation("activate");
  const disableMutation = useLearningModelStatusMutation("disable");
  const retireMutation = useLearningModelStatusMutation("retire");

  const handleAction = async (action: "activate" | "disable" | "retire") => {
    try {
      if (action === "activate") {
        await activateMutation.mutateAsync(model.model_id);
      } else if (action === "disable") {
        await disableMutation.mutateAsync(model.model_id);
      } else {
        await retireMutation.mutateAsync(model.model_id);
      }
      toast.success("Model updated", `${model.model_key} is now ${action}d.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Model update failed", message);
    }
  };

  const isPending =
    activateMutation.isPending || disableMutation.isPending || retireMutation.isPending;

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Button
        variant={selected ? "default" : "outline"}
        size="sm"
        onClick={() => onSelect(model.model_id)}
      >
        <PencilLine className="mr-1.5 h-3.5 w-3.5" />
        Manage
      </Button>
      {model.status !== "active" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("activate")}
        >
          <Play className="mr-1.5 h-3.5 w-3.5" />
          Activate
        </Button>
      ) : null}
      {model.status !== "disabled" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("disable")}
        >
          <Pause className="mr-1.5 h-3.5 w-3.5" />
          Disable
        </Button>
      ) : null}
      {model.status !== "retired" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("retire")}
        >
          <Square className="mr-1.5 h-3.5 w-3.5" />
          Retire
        </Button>
      ) : null}
    </div>
  );
}

function RolloutActionButtons({
  rollout,
  selected,
  onSelect,
}: {
  rollout: LearningRolloutStatus;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  const toast = useToastStore();
  const pauseMutation = useRolloutStatusActionMutation("pause");
  const resumeMutation = useRolloutStatusActionMutation("resume");
  const completeMutation = useRolloutStatusActionMutation("complete");

  const handleAction = async (action: "pause" | "resume" | "complete") => {
    try {
      if (action === "pause") {
        await pauseMutation.mutateAsync(rollout.id);
      } else if (action === "resume") {
        await resumeMutation.mutateAsync(rollout.id);
      } else {
        await completeMutation.mutateAsync(rollout.id);
      }
      toast.success("Rollout updated", `${rollout.model_key} is now ${action}d.`);
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Rollout update failed", message);
    }
  };

  const isPending =
    pauseMutation.isPending || resumeMutation.isPending || completeMutation.isPending;

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Button
        variant={selected ? "default" : "outline"}
        size="sm"
        onClick={() => onSelect(rollout.id)}
      >
        <PencilLine className="mr-1.5 h-3.5 w-3.5" />
        Inspect
      </Button>
      {rollout.status === "active" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("pause")}
        >
          <Pause className="mr-1.5 h-3.5 w-3.5" />
          Pause
        </Button>
      ) : null}
      {rollout.status === "paused" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("resume")}
        >
          <Play className="mr-1.5 h-3.5 w-3.5" />
          Resume
        </Button>
      ) : null}
      {rollout.status === "active" || rollout.status === "paused" ? (
        <Button
          variant="outline"
          size="sm"
          disabled={isPending}
          onClick={() => void handleAction("complete")}
        >
          <Square className="mr-1.5 h-3.5 w-3.5" />
          Complete
        </Button>
      ) : null}
    </div>
  );
}

export default function LearningLoopPage() {
  const [windowKey, setWindowKey] = useState<WindowKey>("30d");
  const [selectedManagedModelId, setSelectedManagedModelId] = useState<string>();
  const [selectedRolloutId, setSelectedRolloutId] = useState<string>();
  const [newModelKey, setNewModelKey] = useState("");
  const [newModelScope, setNewModelScope] = useState<ModelStrategyScope>("arb");
  const [newModelTarget, setNewModelTarget] = useState("open_success_probability");
  const [newModelType, setNewModelType] =
    useState<ModelType>("trained_linear_probability_v1");
  const [newModelVersion, setNewModelVersion] = useState("v1");
  const [newModelStatus, setNewModelStatus] = useState<ModelStatus>("shadow");
  const [newModelFeatureView, setNewModelFeatureView] =
    useState("canonical_arb_learning_attempts");
  const [newModelArtifactUri, setNewModelArtifactUri] = useState("");
  const [editModelKey, setEditModelKey] = useState("");
  const [editModelScope, setEditModelScope] = useState<ModelStrategyScope>("arb");
  const [editModelTarget, setEditModelTarget] = useState("open_success_probability");
  const [editModelType, setEditModelType] =
    useState<ModelType>("trained_linear_probability_v1");
  const [editModelVersion, setEditModelVersion] = useState("v1");
  const [editModelStatus, setEditModelStatus] = useState<ModelStatus>("shadow");
  const [editModelFeatureView, setEditModelFeatureView] =
    useState("canonical_arb_learning_attempts");
  const [editModelArtifactUri, setEditModelArtifactUri] = useState("");
  const [createModelId, setCreateModelId] = useState("");
  const [createRolloutMode, setCreateRolloutMode] = useState<RolloutMode>("canary");
  const [createAuthorityLevel, setCreateAuthorityLevel] =
    useState<AuthorityLevel>("tail_reject");
  const [createBaselineWindowHours, setCreateBaselineWindowHours] = useState("24");
  const [createThreshold, setCreateThreshold] = useState("0.6");
  const [createMaxSizeReductionPct, setCreateMaxSizeReductionPct] = useState("0.2");
  const [createMaxFailureRate, setCreateMaxFailureRate] = useState("0.15");
  const [createMaxOneLeggedRate, setCreateMaxOneLeggedRate] = useState("0.05");
  const [createMaxDrawdownPct, setCreateMaxDrawdownPct] = useState("0.03");
  const [createMaxLatencyP90Ms, setCreateMaxLatencyP90Ms] = useState("900");
  const [createMinEdgeCaptureRatio, setCreateMinEdgeCaptureRatio] = useState("0.6");
  const [editRolloutMode, setEditRolloutMode] = useState<RolloutMode>("canary");
  const [editAuthorityLevel, setEditAuthorityLevel] =
    useState<AuthorityLevel>("tail_reject");
  const [editBaselineWindowHours, setEditBaselineWindowHours] = useState("24");
  const [editThreshold, setEditThreshold] = useState("");
  const [editMaxSizeReductionPct, setEditMaxSizeReductionPct] = useState("");
  const [editMaxFailureRate, setEditMaxFailureRate] = useState("");
  const [editMaxOneLeggedRate, setEditMaxOneLeggedRate] = useState("");
  const [editMaxDrawdownPct, setEditMaxDrawdownPct] = useState("");
  const [editMaxLatencyP90Ms, setEditMaxLatencyP90Ms] = useState("");
  const [editMinEdgeCaptureRatio, setEditMinEdgeCaptureRatio] = useState("");

  const user = useAuthStore((state) => state.user);
  const toast = useToastStore();
  const isPlatformAdmin = user?.role === "PlatformAdmin";

  const params = useMemo(() => {
    const days = WINDOWS[windowKey].days;
    return {
      from: new Date(Date.now() - days * 24 * 60 * 60 * 1000).toISOString(),
      limit: 12,
    };
  }, [windowKey]);

  const { data, isLoading } = useLearningOverviewQuery(params);
  const { data: rolloutDetail, isLoading: isRolloutDetailLoading } =
    useLearningRolloutDetailQuery(selectedRolloutId, { limit: 24 });
  const createModelMutation = useCreateLearningModelMutation();
  const updateModelMutation = useUpdateLearningModelMutation();
  const createRolloutMutation = useCreateLearningRolloutMutation();
  const updateRolloutMutation = useUpdateLearningRolloutMutation();

  useEffect(() => {
    if (!data || data.rollouts.length === 0) {
      setSelectedRolloutId(undefined);
      return;
    }
    const rolloutExists = data.rollouts.some((rollout) => rollout.id === selectedRolloutId);
    if (!selectedRolloutId || !rolloutExists) {
      setSelectedRolloutId(data.rollouts[0].id);
    }
  }, [data, selectedRolloutId]);

  useEffect(() => {
    if (!createModelId && data?.models.length) {
      setCreateModelId(data.models[0].model_id);
    }
  }, [createModelId, data?.models]);

  useEffect(() => {
    if (!data || data.models.length === 0) {
      setSelectedManagedModelId(undefined);
      return;
    }
    const exists = data.models.some((model) => model.model_id === selectedManagedModelId);
    if (!selectedManagedModelId || !exists) {
      setSelectedManagedModelId(data.models[0].model_id);
    }
  }, [data, selectedManagedModelId]);

  useEffect(() => {
    setNewModelFeatureView(featureViewForScope(newModelScope));
    const nextTarget = targetOptionsForScope(newModelScope)[0];
    if (!targetOptionsForScope(newModelScope).includes(newModelTarget)) {
      setNewModelTarget(nextTarget);
    }
  }, [newModelScope, newModelTarget]);

  useEffect(() => {
    if (!rolloutDetail) return;
    const bounds = rolloutDetail.rollout.bounds;
    const guardrails = rolloutDetail.rollout.guardrails;
    setEditRolloutMode(rolloutDetail.rollout.rollout_mode as RolloutMode);
    setEditAuthorityLevel(rolloutDetail.rollout.authority_level as AuthorityLevel);
    setEditBaselineWindowHours(String(rolloutDetail.rollout.baseline_window_hours));
    setEditThreshold(asNumber(bounds.threshold));
    setEditMaxSizeReductionPct(asNumber(bounds.max_size_reduction_pct));
    setEditMaxFailureRate(asNumber(guardrails.max_failure_rate));
    setEditMaxOneLeggedRate(asNumber(guardrails.max_one_legged_rate));
    setEditMaxDrawdownPct(asNumber(guardrails.max_drawdown_pct));
    setEditMaxLatencyP90Ms(asNumber(guardrails.max_latency_p90_ms));
    setEditMinEdgeCaptureRatio(asNumber(guardrails.min_edge_capture_ratio));
  }, [rolloutDetail]);

  const managedModel = data?.models.find((model) => model.model_id === selectedManagedModelId);

  useEffect(() => {
    if (!managedModel) return;
    setEditModelKey(managedModel.model_key);
    setEditModelScope(managedModel.strategy_scope as ModelStrategyScope);
    setEditModelTarget(managedModel.target);
    setEditModelType(managedModel.model_type as ModelType);
    setEditModelVersion(managedModel.version);
    setEditModelStatus(managedModel.status as ModelStatus);
    setEditModelFeatureView(managedModel.feature_view);
    setEditModelArtifactUri(managedModel.artifact_uri ?? "");
  }, [managedModel]);

  useEffect(() => {
    if (!targetOptionsForScope(editModelScope).includes(editModelTarget)) {
      setEditModelTarget(targetOptionsForScope(editModelScope)[0]);
    }
  }, [editModelScope, editModelTarget]);

  const arbCoverage = data
    ? formatRate(data.datasets.arb_realized_outcomes, data.datasets.arb_attempts)
    : "—";
  const quantCoverage = data
    ? formatRate(data.datasets.quant_realized_outcomes, data.datasets.quant_decisions)
    : "—";

  const selectedModel = data?.models.find((model) => model.model_id === createModelId);

  const handleCreateModel = async () => {
    if (!newModelKey.trim()) {
      toast.warning("Missing model key", "Add a unique model key before creating a model.");
      return;
    }

    try {
      await createModelMutation.mutateAsync({
        model_key: newModelKey.trim(),
        strategy_scope: newModelScope,
        target: newModelTarget,
        model_type: newModelType,
        version: newModelVersion,
        status: newModelStatus,
        feature_view: newModelFeatureView,
        artifact_uri: newModelArtifactUri.trim() || undefined,
        metrics: {},
      });
      toast.success("Model created", "The model registry has been updated.");
      setNewModelKey("");
      setNewModelArtifactUri("");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Create model failed", message);
    }
  };

  const handleUpdateModel = async () => {
    if (!managedModel) return;

    try {
      await updateModelMutation.mutateAsync({
        modelId: managedModel.model_id,
        params: {
          model_key: editModelKey.trim(),
          strategy_scope: editModelScope,
          target: editModelTarget,
          model_type: editModelType,
          version: editModelVersion,
          status: editModelStatus,
          feature_view: editModelFeatureView,
          artifact_uri: editModelArtifactUri.trim() || undefined,
          metrics: managedModel.metrics,
        },
      });
      toast.success("Model updated", "The trained-model configuration was saved.");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Update model failed", message);
    }
  };

  const handleCreateRollout = async () => {
    if (!createModelId) {
      toast.warning("Pick a model", "Choose a registered model before starting a rollout.");
      return;
    }

    try {
      await createRolloutMutation.mutateAsync({
        model_id: createModelId,
        rollout_mode: createRolloutMode,
        authority_level: createAuthorityLevel,
        baseline_window_hours: parseNumberInput(createBaselineWindowHours),
        bounds: buildBounds(createThreshold, createMaxSizeReductionPct),
        guardrails: buildGuardrails(
          createMaxFailureRate,
          createMaxOneLeggedRate,
          createMaxDrawdownPct,
          createMaxLatencyP90Ms,
          createMinEdgeCaptureRatio,
        ),
      });
      toast.success("Rollout created", "The new model rollout is now visible in guardrails.");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Create rollout failed", message);
    }
  };

  const handleUpdateRollout = async () => {
    if (!selectedRolloutId) return;
    try {
      await updateRolloutMutation.mutateAsync({
        rolloutId: selectedRolloutId,
        params: {
          rollout_mode: editRolloutMode,
          authority_level: editAuthorityLevel,
          baseline_window_hours: parseNumberInput(editBaselineWindowHours),
          bounds: buildBounds(editThreshold, editMaxSizeReductionPct),
          guardrails: buildGuardrails(
            editMaxFailureRate,
            editMaxOneLeggedRate,
            editMaxDrawdownPct,
            editMaxLatencyP90Ms,
            editMinEdgeCaptureRatio,
          ),
        },
      });
      toast.success("Rollout updated", "The rollout limits and guardrails were saved.");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Request failed";
      toast.error("Update rollout failed", message);
    }
  };

  return (
    <div className="space-y-5 sm:space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex items-center gap-3">
          <BrainCircuit className="h-6 w-6 text-muted-foreground" />
          <div>
            <h1 className="text-2xl font-bold">Learning Loop</h1>
            <p className="text-sm text-muted-foreground">
              Dataset coverage, shadow models, offline evals, and bounded rollouts
            </p>
          </div>
        </div>
        <LiveIndicator />
      </div>

      <PageIntro
        title="What this page proves"
        description="A self-improving model only works if we can join predictions to clean outcomes, score them offline, and enforce rollback rules in production."
        bullets={[
          "Dataset readiness shows whether arb attempts and quant decisions are learnable instead of just logged.",
          "Models stay in shadow until offline evaluations and live-forward evidence justify bounded authority.",
          "Rollout guardrails are the kill-switches for failure-rate, drawdown, and latency drift.",
        ]}
      />

      <Tabs value={windowKey} onValueChange={(value) => setWindowKey(value as WindowKey)}>
        <div className="overflow-x-auto">
          <TabsList className="w-max">
            {(Object.keys(WINDOWS) as WindowKey[]).map((key) => (
              <TabsTrigger key={key} value={key}>
                {WINDOWS[key].label}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
      </Tabs>

      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
        <MetricCard
          title="Arb Attempts"
          value={data ? String(data.datasets.arb_attempts) : "—"}
          changeLabel={data ? `${arbCoverage} realized` : undefined}
          trend="neutral"
        />
        <MetricCard
          title="Quant Decisions"
          value={data ? String(data.datasets.quant_decisions) : "—"}
          changeLabel={data ? `${quantCoverage} realized` : undefined}
          trend="neutral"
        />
        <MetricCard
          title="Shadow Predictions"
          value={data ? String(data.datasets.shadow_predictions) : "—"}
          trend={(data?.datasets.shadow_predictions ?? 0) > 0 ? "up" : "neutral"}
        />
        <MetricCard
          title="Active Rollouts"
          value={data ? String(data.datasets.active_rollouts) : "—"}
          trend={(data?.datasets.active_rollouts ?? 0) > 0 ? "up" : "neutral"}
        />
        <MetricCard
          title="One-Legged Arb Fails"
          value={data ? String(data.datasets.arb_one_legged_failures) : "—"}
          trend={(data?.datasets.arb_one_legged_failures ?? 0) === 0 ? "up" : "down"}
        />
        <MetricCard
          title="Quant Executed"
          value={data ? String(data.datasets.quant_executed_decisions) : "—"}
          trend="neutral"
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-[1.25fr_1fr]">
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <BrainCircuit className="h-4 w-4 text-muted-foreground" />
              <span>Model Registry</span>
              <InfoTooltip content="Models should move draft → shadow → canary → active only after offline replay and shadow-mode evidence." />
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <p className="text-sm text-muted-foreground">Loading model registry…</p>
            ) : !data || data.models.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No learning models registered yet. The canonical datasets are ready for the first shadow models.
              </p>
            ) : (
              <div className="space-y-3">
                {data.models.map((model) => (
                  <div
                    key={model.model_id}
                    className="rounded-xl border border-border/60 p-4"
                  >
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                      <div className="space-y-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <p className="text-sm font-semibold">{model.model_key}</p>
                          <Badge
                            variant="outline"
                            className={cn("text-xs", statusBadgeClass(model.status))}
                          >
                            {model.status}
                          </Badge>
                          <Badge variant="outline" className="text-xs">
                            {model.strategy_scope}
                          </Badge>
                        </div>
                        <p className="text-xs text-muted-foreground">
                          {model.target} • {model.model_type} • v{model.version}
                        </p>
                      </div>
                      <div className="space-y-2 text-xs text-muted-foreground">
                        <div>{model.shadow_predictions} shadow predictions</div>
                        {isPlatformAdmin ? (
                          <ModelActionButtons
                            model={model}
                            selected={model.model_id === selectedManagedModelId}
                            onSelect={setSelectedManagedModelId}
                          />
                        ) : null}
                      </div>
                    </div>
                    <div className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
                      <div>Feature view: {model.feature_view}</div>
                      <div>
                        Activated: {model.activated_at ? new Date(model.activated_at).toLocaleString() : "Not yet"}
                      </div>
                      <div className="sm:col-span-2">
                        Artifact: {model.artifact_uri ?? "embedded baseline / metrics artifact"}
                      </div>
                    </div>
                    <p className="mt-3 text-xs text-muted-foreground">
                      {summarizeObject(model.metrics)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <FlaskConical className="h-4 w-4 text-muted-foreground" />
              <span>Offline Evaluations</span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isLoading ? (
              <p className="text-sm text-muted-foreground">Loading offline evals…</p>
            ) : !data || data.offline_evaluations.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No offline evaluations recorded yet. This is the gating step before any live rollout.
              </p>
            ) : (
              <div className="space-y-3">
                {data.offline_evaluations.map((evaluation) => (
                  <div
                    key={evaluation.id}
                    className="rounded-xl border border-border/60 p-4"
                  >
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="text-sm font-semibold">{evaluation.model_key}</p>
                      <Badge variant="outline" className="text-xs">
                        {evaluation.evaluation_scope}
                      </Badge>
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {evaluation.dataset_name} • {new Date(evaluation.created_at).toLocaleString()}
                    </p>
                    <p className="mt-3 text-xs text-muted-foreground">
                      {summarizeObject(evaluation.metrics)}
                    </p>
                    <p className="mt-2 text-xs text-muted-foreground">
                      Policy: {summarizeObject(evaluation.decision_policy)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {isPlatformAdmin ? (
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Cpu className="h-4 w-4 text-muted-foreground" />
              <span>Model Registry Admin</span>
              <InfoTooltip content="Register trained artifacts here, then activate them once offline replay and shadow evidence are acceptable." />
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-4 xl:grid-cols-2">
              <div className="rounded-xl border border-border/60 p-4">
                <p className="text-sm font-medium">Create model</p>
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <div className="grid gap-2">
                    <Label>Model Key</Label>
                    <Input value={newModelKey} onChange={(event) => setNewModelKey(event.target.value)} />
                  </div>
                  <div className="grid gap-2">
                    <Label>Strategy Scope</Label>
                    <Select
                      value={newModelScope}
                      onValueChange={(value) => setNewModelScope(value as ModelStrategyScope)}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="arb">arb</SelectItem>
                        <SelectItem value="quant">quant</SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-2">
                    <Label>Target</Label>
                    <Select value={newModelTarget} onValueChange={setNewModelTarget}>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {targetOptionsForScope(newModelScope).map((target) => (
                          <SelectItem key={target} value={target}>
                            {target}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-2">
                    <Label>Model Type</Label>
                    <Select value={newModelType} onValueChange={(value) => setNewModelType(value as ModelType)}>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {MODEL_TYPES.map((modelType) => (
                          <SelectItem key={modelType} value={modelType}>
                            {modelType}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-2">
                    <Label>Version</Label>
                    <Input value={newModelVersion} onChange={(event) => setNewModelVersion(event.target.value)} />
                  </div>
                  <div className="grid gap-2">
                    <Label>Status</Label>
                    <Select value={newModelStatus} onValueChange={(value) => setNewModelStatus(value as ModelStatus)}>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {MODEL_STATUSES.map((status) => (
                          <SelectItem key={status} value={status}>
                            {status}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <div className="grid gap-2 sm:col-span-2">
                    <Label>Feature View</Label>
                    <Input value={newModelFeatureView} onChange={(event) => setNewModelFeatureView(event.target.value)} />
                  </div>
                  <div className="grid gap-2 sm:col-span-2">
                    <Label>Artifact Path</Label>
                    <Input
                      value={newModelArtifactUri}
                      onChange={(event) => setNewModelArtifactUri(event.target.value)}
                      placeholder="/absolute/path/to/model.json"
                    />
                    <p className="text-xs text-muted-foreground">
                      Supported trained artifacts are local JSON files or embedded `metrics.artifact`.
                    </p>
                  </div>
                </div>
                <div className="mt-4 flex justify-end">
                  <Button onClick={() => void handleCreateModel()} disabled={createModelMutation.isPending}>
                    {createModelMutation.isPending ? "Creating…" : "Create Model"}
                  </Button>
                </div>
              </div>

              <div className="rounded-xl border border-border/60 p-4">
                <p className="text-sm font-medium">Edit selected model</p>
                {!managedModel ? (
                  <p className="mt-3 text-sm text-muted-foreground">
                    Select a model from the registry to edit its artifact path or lifecycle.
                  </p>
                ) : (
                  <>
                    <div className="mt-3 grid gap-3 sm:grid-cols-2">
                      <div className="grid gap-2">
                        <Label>Model Key</Label>
                        <Input value={editModelKey} onChange={(event) => setEditModelKey(event.target.value)} />
                      </div>
                      <div className="grid gap-2">
                        <Label>Strategy Scope</Label>
                        <Select
                          value={editModelScope}
                          onValueChange={(value) => {
                            setEditModelScope(value as ModelStrategyScope);
                            setEditModelFeatureView(featureViewForScope(value as ModelStrategyScope));
                          }}
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="arb">arb</SelectItem>
                            <SelectItem value="quant">quant</SelectItem>
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2">
                        <Label>Target</Label>
                        <Select value={editModelTarget} onValueChange={setEditModelTarget}>
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {targetOptionsForScope(editModelScope).map((target) => (
                              <SelectItem key={target} value={target}>
                                {target}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2">
                        <Label>Model Type</Label>
                        <Select value={editModelType} onValueChange={(value) => setEditModelType(value as ModelType)}>
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {MODEL_TYPES.map((modelType) => (
                              <SelectItem key={modelType} value={modelType}>
                                {modelType}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2">
                        <Label>Version</Label>
                        <Input value={editModelVersion} onChange={(event) => setEditModelVersion(event.target.value)} />
                      </div>
                      <div className="grid gap-2">
                        <Label>Status</Label>
                        <Select value={editModelStatus} onValueChange={(value) => setEditModelStatus(value as ModelStatus)}>
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {MODEL_STATUSES.map((status) => (
                              <SelectItem key={status} value={status}>
                                {status}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2 sm:col-span-2">
                        <Label>Feature View</Label>
                        <Input
                          value={editModelFeatureView}
                          onChange={(event) => setEditModelFeatureView(event.target.value)}
                        />
                      </div>
                      <div className="grid gap-2 sm:col-span-2">
                        <Label>Artifact Path</Label>
                        <Input
                          value={editModelArtifactUri}
                          onChange={(event) => setEditModelArtifactUri(event.target.value)}
                        />
                      </div>
                    </div>
                    <div className="mt-4 flex justify-end">
                      <Button onClick={() => void handleUpdateModel()} disabled={updateModelMutation.isPending}>
                        {updateModelMutation.isPending ? "Saving…" : "Save Model"}
                      </Button>
                    </div>
                  </>
                )}
              </div>
            </div>
          </CardContent>
        </Card>
      ) : null}

      {isPlatformAdmin ? (
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <Play className="h-4 w-4 text-muted-foreground" />
              <span>Rollout Launcher</span>
              <InfoTooltip content="Create bounded rollouts without writing SQL. Start narrow, observe live drift, and only then widen authority." />
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
              <div className="grid gap-2">
                <Label>Model</Label>
                <Select value={createModelId} onValueChange={setCreateModelId}>
                  <SelectTrigger>
                    <SelectValue placeholder="Select a model" />
                  </SelectTrigger>
                  <SelectContent>
                    {data?.models.map((model) => (
                      <SelectItem key={model.model_id} value={model.model_id}>
                        {model.model_key}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground">
                  {selectedModel
                    ? `${selectedModel.strategy_scope} • ${selectedModel.target} • ${selectedModel.status}`
                    : "Choose a registered model to start a rollout."}
                </p>
              </div>
              <div className="grid gap-2">
                <Label>Rollout Mode</Label>
                <Select
                  value={createRolloutMode}
                  onValueChange={(value) => {
                    const nextMode = value as RolloutMode;
                    setCreateRolloutMode(nextMode);
                    if (nextMode === "shadow") {
                      setCreateAuthorityLevel("observe");
                    } else if (createAuthorityLevel === "observe") {
                      setCreateAuthorityLevel(rolloutDefaultAuthority(nextMode));
                    }
                  }}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {ROLLOUT_MODES.map((mode) => (
                      <SelectItem key={mode} value={mode}>
                        {mode}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label>Authority</Label>
                <Select value={createAuthorityLevel} onValueChange={(value) => setCreateAuthorityLevel(value as AuthorityLevel)}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {AUTHORITY_OPTIONS.map((authority) => (
                      <SelectItem
                        key={authority}
                        value={authority}
                        disabled={createRolloutMode === "shadow" && authority !== "observe"}
                      >
                        {authority}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label>Baseline Window (hours)</Label>
                <Input
                  value={createBaselineWindowHours}
                  onChange={(event) => setCreateBaselineWindowHours(event.target.value)}
                  inputMode="numeric"
                />
              </div>
            </div>

            <div className="grid gap-4 xl:grid-cols-2">
              <div className="rounded-xl border border-border/60 p-4">
                <p className="text-sm font-medium">Bounds</p>
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <div className="grid gap-2">
                    <Label>Decision Threshold</Label>
                    <Input value={createThreshold} onChange={(event) => setCreateThreshold(event.target.value)} />
                  </div>
                  <div className="grid gap-2">
                    <Label>Max Size Reduction</Label>
                    <Input
                      value={createMaxSizeReductionPct}
                      onChange={(event) => setCreateMaxSizeReductionPct(event.target.value)}
                    />
                  </div>
                </div>
              </div>
              <div className="rounded-xl border border-border/60 p-4">
                <p className="text-sm font-medium">Guardrails</p>
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <div className="grid gap-2">
                    <Label>Max Failure Rate</Label>
                    <Input value={createMaxFailureRate} onChange={(event) => setCreateMaxFailureRate(event.target.value)} />
                  </div>
                  <div className="grid gap-2">
                    <Label>Max One-Legged Rate</Label>
                    <Input
                      value={createMaxOneLeggedRate}
                      onChange={(event) => setCreateMaxOneLeggedRate(event.target.value)}
                    />
                  </div>
                  <div className="grid gap-2">
                    <Label>Max Drawdown</Label>
                    <Input value={createMaxDrawdownPct} onChange={(event) => setCreateMaxDrawdownPct(event.target.value)} />
                  </div>
                  <div className="grid gap-2">
                    <Label>Max Latency P90 (ms)</Label>
                    <Input
                      value={createMaxLatencyP90Ms}
                      onChange={(event) => setCreateMaxLatencyP90Ms(event.target.value)}
                    />
                  </div>
                  <div className="grid gap-2">
                    <Label>Min Edge Capture Ratio</Label>
                    <Input
                      value={createMinEdgeCaptureRatio}
                      onChange={(event) => setCreateMinEdgeCaptureRatio(event.target.value)}
                    />
                  </div>
                </div>
              </div>
            </div>

            <div className="flex justify-end">
              <Button onClick={() => void handleCreateRollout()} disabled={createRolloutMutation.isPending}>
                {createRolloutMutation.isPending ? "Creating…" : "Create Rollout"}
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : null}

      <Card>
        <CardHeader className="pb-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <ShieldCheck className="h-4 w-4 text-muted-foreground" />
            <span>Rollout Guardrails</span>
            <InfoTooltip content="Rollouts should stay bounded: small authority, explicit limits, and automatic rollback when live metrics drift." />
          </CardTitle>
        </CardHeader>
        <CardContent>
          {isLoading ? (
            <p className="text-sm text-muted-foreground">Loading rollout state…</p>
          ) : !data || data.rollouts.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No model rollouts are active. The system is still in measurement and shadow-mode only.
            </p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[1180px]">
                <thead>
                  <tr className="border-b border-border/60 text-left text-xs uppercase tracking-wide text-muted-foreground">
                    <th className="px-3 py-2 font-medium">Model</th>
                    <th className="px-3 py-2 font-medium">Mode</th>
                    <th className="px-3 py-2 font-medium">Authority</th>
                    <th className="px-3 py-2 font-medium">Guardrail</th>
                    <th className="px-3 py-2 font-medium">Failure</th>
                    <th className="px-3 py-2 font-medium">One-Legged</th>
                    <th className="px-3 py-2 font-medium">Drawdown</th>
                    <th className="px-3 py-2 font-medium">Latency P90</th>
                    <th className="px-3 py-2 font-medium">Edge Capture</th>
                    <th className="px-3 py-2 font-medium">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {data.rollouts.map((rollout) => {
                    const selected = rollout.id === selectedRolloutId;
                    return (
                      <tr
                        key={rollout.id}
                        className={cn(
                          "border-b border-border/60 last:border-0",
                          selected && "bg-muted/40",
                        )}
                      >
                        <td className="px-3 py-3 text-sm">
                          <div className="font-medium">{rollout.model_key}</div>
                          <div className="text-xs text-muted-foreground">
                            {rollout.strategy_scope} • {rollout.status}
                          </div>
                        </td>
                        <td className="px-3 py-3 text-sm">{rollout.rollout_mode}</td>
                        <td className="px-3 py-3 text-sm">{rollout.authority_level}</td>
                        <td className="px-3 py-3 text-sm">
                          <Badge
                            variant="outline"
                            className={cn(
                              "text-xs",
                              statusBadgeClass(rollout.latest_guardrail_state ?? rollout.status),
                            )}
                          >
                            {rollout.latest_guardrail_state ?? "no data"}
                          </Badge>
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">
                          {formatPct(rollout.latest_failure_rate)}
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">
                          {formatPct(rollout.latest_one_legged_rate)}
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">
                          {formatPct(rollout.latest_drawdown_pct)}
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">
                          {formatMs(rollout.latest_latency_p90_ms)}
                        </td>
                        <td className="px-3 py-3 text-sm tabular-nums">
                          {rollout.latest_edge_capture_ratio != null
                            ? `${rollout.latest_edge_capture_ratio.toFixed(2)}x`
                            : "—"}
                        </td>
                        <td className="px-3 py-3 text-sm">
                          <RolloutActionButtons
                            rollout={rollout}
                            selected={selected}
                            onSelect={setSelectedRolloutId}
                          />
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-4 xl:grid-cols-[1.1fr_0.9fr]">
        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <PencilLine className="h-4 w-4 text-muted-foreground" />
              <span>Selected Rollout</span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isRolloutDetailLoading ? (
              <p className="text-sm text-muted-foreground">Loading rollout detail…</p>
            ) : !rolloutDetail ? (
              <p className="text-sm text-muted-foreground">
                Select a rollout to inspect its latest guardrails and observation history.
              </p>
            ) : (
              <div className="space-y-4">
                <div className="rounded-xl border border-border/60 p-4">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-semibold">{rolloutDetail.rollout.model_key}</p>
                    <Badge variant="outline" className={cn("text-xs", statusBadgeClass(rolloutDetail.rollout.status))}>
                      {rolloutDetail.rollout.status}
                    </Badge>
                    <Badge variant="outline" className="text-xs">
                      {rolloutDetail.rollout.strategy_scope}
                    </Badge>
                  </div>
                  <p className="mt-2 text-xs text-muted-foreground">
                    Started {new Date(rolloutDetail.rollout.started_at).toLocaleString()}
                    {rolloutDetail.rollout.ended_at
                      ? ` • Ended ${new Date(rolloutDetail.rollout.ended_at).toLocaleString()}`
                      : ""}
                  </p>
                  <p className="mt-3 text-xs text-muted-foreground">
                    Bounds: {summarizeObject(rolloutDetail.rollout.bounds)}
                  </p>
                  <p className="mt-2 text-xs text-muted-foreground">
                    Guardrails: {summarizeObject(rolloutDetail.rollout.guardrails)}
                  </p>
                  {rolloutDetail.rollout.rollback_reason ? (
                    <p className="mt-2 text-xs text-red-600">
                      Rollback reason: {rolloutDetail.rollout.rollback_reason}
                    </p>
                  ) : null}
                </div>

                {isPlatformAdmin &&
                (rolloutDetail.rollout.status === "active" ||
                  rolloutDetail.rollout.status === "paused") ? (
                  <div className="rounded-xl border border-border/60 p-4">
                    <p className="text-sm font-medium">Edit rollout</p>
                    <div className="mt-3 grid gap-4 md:grid-cols-2 xl:grid-cols-4">
                      <div className="grid gap-2">
                        <Label>Rollout Mode</Label>
                        <Select
                          value={editRolloutMode}
                          onValueChange={(value) => {
                            const nextMode = value as RolloutMode;
                            setEditRolloutMode(nextMode);
                            if (nextMode === "shadow") {
                              setEditAuthorityLevel("observe");
                            }
                          }}
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {ROLLOUT_MODES.map((mode) => (
                              <SelectItem key={mode} value={mode}>
                                {mode}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2">
                        <Label>Authority</Label>
                        <Select
                          value={editAuthorityLevel}
                          onValueChange={(value) => setEditAuthorityLevel(value as AuthorityLevel)}
                        >
                          <SelectTrigger>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {AUTHORITY_OPTIONS.map((authority) => (
                              <SelectItem
                                key={authority}
                                value={authority}
                                disabled={editRolloutMode === "shadow" && authority !== "observe"}
                              >
                                {authority}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                      <div className="grid gap-2">
                        <Label>Baseline Window (hours)</Label>
                        <Input
                          value={editBaselineWindowHours}
                          onChange={(event) => setEditBaselineWindowHours(event.target.value)}
                        />
                      </div>
                    </div>
                    <div className="mt-4 grid gap-4 xl:grid-cols-2">
                      <div className="rounded-xl border border-border/60 p-4">
                        <p className="text-sm font-medium">Bounds</p>
                        <div className="mt-3 grid gap-3 sm:grid-cols-2">
                          <div className="grid gap-2">
                            <Label>Decision Threshold</Label>
                            <Input value={editThreshold} onChange={(event) => setEditThreshold(event.target.value)} />
                          </div>
                          <div className="grid gap-2">
                            <Label>Max Size Reduction</Label>
                            <Input
                              value={editMaxSizeReductionPct}
                              onChange={(event) => setEditMaxSizeReductionPct(event.target.value)}
                            />
                          </div>
                        </div>
                      </div>
                      <div className="rounded-xl border border-border/60 p-4">
                        <p className="text-sm font-medium">Guardrails</p>
                        <div className="mt-3 grid gap-3 sm:grid-cols-2">
                          <div className="grid gap-2">
                            <Label>Max Failure Rate</Label>
                            <Input value={editMaxFailureRate} onChange={(event) => setEditMaxFailureRate(event.target.value)} />
                          </div>
                          <div className="grid gap-2">
                            <Label>Max One-Legged Rate</Label>
                            <Input
                              value={editMaxOneLeggedRate}
                              onChange={(event) => setEditMaxOneLeggedRate(event.target.value)}
                            />
                          </div>
                          <div className="grid gap-2">
                            <Label>Max Drawdown</Label>
                            <Input value={editMaxDrawdownPct} onChange={(event) => setEditMaxDrawdownPct(event.target.value)} />
                          </div>
                          <div className="grid gap-2">
                            <Label>Max Latency P90 (ms)</Label>
                            <Input
                              value={editMaxLatencyP90Ms}
                              onChange={(event) => setEditMaxLatencyP90Ms(event.target.value)}
                            />
                          </div>
                          <div className="grid gap-2">
                            <Label>Min Edge Capture Ratio</Label>
                            <Input
                              value={editMinEdgeCaptureRatio}
                              onChange={(event) => setEditMinEdgeCaptureRatio(event.target.value)}
                            />
                          </div>
                        </div>
                      </div>
                    </div>
                    <div className="mt-4 flex justify-end">
                      <Button onClick={() => void handleUpdateRollout()} disabled={updateRolloutMutation.isPending}>
                        {updateRolloutMutation.isPending ? "Saving…" : "Save Rollout"}
                      </Button>
                    </div>
                  </div>
                ) : null}
              </div>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-base">
              <ShieldCheck className="h-4 w-4 text-muted-foreground" />
              <span>Observation History</span>
            </CardTitle>
          </CardHeader>
          <CardContent>
            {isRolloutDetailLoading ? (
              <p className="text-sm text-muted-foreground">Loading observations…</p>
            ) : !rolloutDetail || rolloutDetail.observations.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No observation samples yet. Once the observer job runs, each cycle lands here.
              </p>
            ) : (
              <div className="space-y-3">
                {rolloutDetail.observations.map((observation) => (
                  <div key={observation.id} className="rounded-xl border border-border/60 p-4">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <p className="text-sm font-medium">
                        {new Date(observation.observed_at).toLocaleString()}
                      </p>
                      <Badge
                        variant="outline"
                        className={cn("text-xs", statusBadgeClass(observation.guardrail_state))}
                      >
                        {observation.guardrail_state}
                      </Badge>
                    </div>
                    <div className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
                      <div>Failure: {formatPct(observation.failure_rate)}</div>
                      <div>One-Legged: {formatPct(observation.one_legged_rate)}</div>
                      <div>Drawdown: {formatPct(observation.drawdown_pct)}</div>
                      <div>Latency P90: {formatMs(observation.latency_p90_ms)}</div>
                      <div>
                        Edge Capture:{" "}
                        {observation.edge_capture_ratio != null
                          ? `${observation.edge_capture_ratio.toFixed(2)}x`
                          : "—"}
                      </div>
                    </div>
                    <p className="mt-3 text-xs text-muted-foreground">
                      {summarizeObject(observation.notes)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
