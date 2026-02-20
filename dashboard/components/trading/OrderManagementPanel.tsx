"use client";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { useOrderQuery, useCancelOrderMutation } from "@/hooks/queries/useOrdersQuery";
import { formatCurrency, formatTimeAgo } from "@/lib/utils";
import { useToastStore } from "@/stores/toast-store";
import { X, ClipboardList } from "lucide-react";
import { cn } from "@/lib/utils";

interface OrderManagementPanelProps {
  recentOrderIds: string[];
}

function OrderRow({ orderId }: { orderId: string }) {
  const { data: order, isLoading } = useOrderQuery(orderId);
  const cancelMutation = useCancelOrderMutation();
  const toast = useToastStore();

  const handleCancel = () => {
    cancelMutation.mutate(orderId, {
      onSuccess: () => toast.success("Order Cancelled"),
      onError: () => toast.error("Cancel Failed", "Could not cancel order"),
    });
  };

  if (isLoading) {
    return (
      <div className="flex items-center gap-3 p-3 border-b">
        <Skeleton className="h-4 w-16" />
        <Skeleton className="h-4 flex-1" />
        <Skeleton className="h-6 w-20" />
      </div>
    );
  }

  if (!order) return null;

  const canCancel = order.status === "Pending" || order.status === "Open";

  const statusColor: Record<string, string> = {
    Pending: "bg-yellow-500/10 text-yellow-600",
    Open: "bg-blue-500/10 text-blue-600",
    PartiallyFilled: "bg-blue-500/10 text-blue-600",
    Filled: "bg-profit/10 text-profit",
    Cancelled: "bg-muted text-muted-foreground",
    Rejected: "bg-loss/10 text-loss",
    Expired: "bg-muted text-muted-foreground",
  };

  return (
    <div className="flex items-center gap-3 p-3 border-b last:border-b-0">
      <Badge className={cn("text-xs", statusColor[order.status] || "")}>
        {order.status}
      </Badge>
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium">
          {order.side} {order.outcome.toUpperCase()}
        </p>
        <p className="text-xs text-muted-foreground">
          {order.filled_quantity}/{order.quantity} filled
          {order.avg_fill_price != null && ` @ ${(order.avg_fill_price * 100).toFixed(1)}¢`}
        </p>
      </div>
      <span className="text-xs text-muted-foreground">
        {formatTimeAgo(order.created_at)}
      </span>
      {canCancel && (
        <Button
          variant="ghost"
          size="sm"
          onClick={handleCancel}
          disabled={cancelMutation.isPending}
        >
          <X className="h-3 w-3" />
        </Button>
      )}
    </div>
  );
}

export function OrderManagementPanel({ recentOrderIds }: OrderManagementPanelProps) {
  if (recentOrderIds.length === 0) {
    return (
      <Card>
        <CardContent className="p-6 text-center">
          <ClipboardList className="h-8 w-8 mx-auto mb-2 text-muted-foreground" />
          <p className="text-sm text-muted-foreground">
            No orders placed this session
          </p>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Session Orders</CardTitle>
      </CardHeader>
      <CardContent className="p-0">
        {recentOrderIds.map((id) => (
          <OrderRow key={id} orderId={id} />
        ))}
      </CardContent>
    </Card>
  );
}
