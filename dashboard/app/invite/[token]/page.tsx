'use client';

import { useEffect, useState } from 'react';
import { useParams, useRouter } from 'next/navigation';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Building2, CheckCircle, XCircle, Loader2 } from 'lucide-react';
import api from '@/lib/api';
import { useAuthStore } from '@/stores/auth-store';
import { useWorkspaceStore } from '@/stores/workspace-store';
import type { InviteInfo } from '@/types/api';

export default function InviteAcceptPage() {
  const params = useParams();
  const router = useRouter();
  const token = params.token as string;
  const { isAuthenticated, user, setAuth } = useAuthStore();
  const { switchWorkspace, fetchWorkspaces } = useWorkspaceStore();

  const [inviteInfo, setInviteInfo] = useState<InviteInfo | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [isAccepting, setIsAccepting] = useState(false);
  const [accepted, setAccepted] = useState(false);

  // Form state for new users
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [name, setName] = useState('');

  useEffect(() => {
    async function fetchInviteInfo() {
      try {
        const info = await api.getInviteInfo(token);
        setInviteInfo(info);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Invalid or expired invite');
      } finally {
        setIsLoading(false);
      }
    }
    fetchInviteInfo();
  }, [token]);

  const handleAccept = async () => {
    setIsAccepting(true);
    setError(null);

    try {
      const response = await api.acceptInvite(token, {
        email: !isAuthenticated ? email : undefined,
        password: !isAuthenticated ? password : undefined,
        name: !isAuthenticated ? name : undefined,
      });

      // If new user, set auth
      if (response.token && response.user) {
        setAuth(response.token, response.user);
      }

      // Switch to the new workspace and refresh workspaces list
      await switchWorkspace(response.workspace_id);
      await fetchWorkspaces();

      setAccepted(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to accept invite');
    } finally {
      setIsAccepting(false);
    }
  };

  const formatRole = (role: string) => {
    return role.charAt(0).toUpperCase() + role.slice(1);
  };

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background">
        <div className="flex flex-col items-center gap-4">
          <Loader2 className="h-8 w-8 animate-spin text-primary" />
          <p className="text-sm text-muted-foreground">Loading invite...</p>
        </div>
      </div>
    );
  }

  if (error && !inviteInfo) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <XCircle className="h-12 w-12 text-destructive mx-auto mb-4" />
            <CardTitle>Invalid Invite</CardTitle>
            <CardDescription>{error}</CardDescription>
          </CardHeader>
          <CardContent>
            <Button className="w-full" onClick={() => router.push('/login')}>
              Go to Login
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (accepted) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <CheckCircle className="h-12 w-12 text-green-500 mx-auto mb-4" />
            <CardTitle>Welcome to {inviteInfo?.workspace_name}!</CardTitle>
            <CardDescription>
              You&apos;ve successfully joined the workspace as a {formatRole(inviteInfo?.role || 'member')}.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Button className="w-full" onClick={() => router.push('/')}>
              Go to Dashboard
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <div className="flex items-center justify-center mb-4">
            <div className="flex h-16 w-16 items-center justify-center rounded-full bg-primary/10">
              <Building2 className="h-8 w-8 text-primary" />
            </div>
          </div>
          <CardTitle>You&apos;re Invited!</CardTitle>
          <CardDescription>
            {inviteInfo?.inviter_email} has invited you to join
          </CardDescription>
          <p className="text-xl font-semibold mt-2">{inviteInfo?.workspace_name}</p>
          <p className="text-sm text-muted-foreground">
            Role: {formatRole(inviteInfo?.role || 'member')}
          </p>
        </CardHeader>
        <CardContent className="space-y-4">
          {error && (
            <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
              {error}
            </div>
          )}

          {isAuthenticated && user?.email === inviteInfo?.email ? (
            // Logged in user matches invite email - can accept directly
            <>
              <p className="text-sm text-center text-muted-foreground">
                You&apos;re signed in as <span className="font-medium">{user?.email}</span>
              </p>
              <Button className="w-full" onClick={handleAccept} disabled={isAccepting}>
                {isAccepting ? (
                  <>
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                    Joining...
                  </>
                ) : (
                  'Accept Invite'
                )}
              </Button>
            </>
          ) : isAuthenticated && user?.email !== inviteInfo?.email ? (
            // Logged in as different user - show warning
            <>
              <div className="rounded-md bg-yellow-500/10 border border-yellow-500/20 p-3 text-sm text-yellow-700 dark:text-yellow-400">
                <p className="font-medium">Email mismatch</p>
                <p className="mt-1">
                  This invite is for <span className="font-medium">{inviteInfo?.email}</span>,
                  but you&apos;re signed in as <span className="font-medium">{user?.email}</span>.
                </p>
              </div>
              <p className="text-sm text-center text-muted-foreground mt-2">
                Sign out and create an account with the invited email, or use the form below.
              </p>

              <div className="space-y-4 mt-4">
                <div className="grid gap-2">
                  <Label htmlFor="name">Name (optional)</Label>
                  <Input
                    id="name"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Your name"
                  />
                </div>

                <div className="grid gap-2">
                  <Label htmlFor="password">Password for new account</Label>
                  <Input
                    id="password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="Min. 8 characters"
                    required
                  />
                </div>

                <Button
                  className="w-full"
                  onClick={handleAccept}
                  disabled={isAccepting || !password}
                >
                  {isAccepting ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      Creating Account...
                    </>
                  ) : (
                    `Create Account for ${inviteInfo?.email}`
                  )}
                </Button>
              </div>
            </>
          ) : (
            <>
              <p className="text-sm text-center text-muted-foreground">
                Create an account to join this workspace
              </p>

              <div className="space-y-4">
                <div className="grid gap-2">
                  <Label htmlFor="email">Email</Label>
                  <Input
                    id="email"
                    type="email"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    placeholder="you@example.com"
                    required
                  />
                </div>

                <div className="grid gap-2">
                  <Label htmlFor="name">Name (optional)</Label>
                  <Input
                    id="name"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="Your name"
                  />
                </div>

                <div className="grid gap-2">
                  <Label htmlFor="password">Password</Label>
                  <Input
                    id="password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="Min. 8 characters"
                    required
                  />
                </div>

                <Button
                  className="w-full"
                  onClick={handleAccept}
                  disabled={isAccepting || !email || !password}
                >
                  {isAccepting ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      Creating Account...
                    </>
                  ) : (
                    'Create Account & Join'
                  )}
                </Button>
              </div>

              <div className="relative">
                <div className="absolute inset-0 flex items-center">
                  <span className="w-full border-t" />
                </div>
                <div className="relative flex justify-center text-xs uppercase">
                  <span className="bg-background px-2 text-muted-foreground">
                    or
                  </span>
                </div>
              </div>

              <Button
                variant="outline"
                className="w-full"
                onClick={() => router.push(`/login?redirect=/invite/${token}`)}
              >
                Sign in with existing account
              </Button>
            </>
          )}

          <p className="text-xs text-center text-muted-foreground">
            This invite expires on{' '}
            {inviteInfo?.expires_at
              ? new Date(inviteInfo.expires_at).toLocaleDateString()
              : 'soon'}
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
