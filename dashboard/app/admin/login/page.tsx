'use client';

import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { ShieldAlert } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { loginSchema, type LoginFormData } from '@/lib/validations';
import { useAuthStore } from '@/stores/auth-store';
import { useToastStore } from '@/stores/toast-store';
import api from '@/lib/api';

export default function AdminLoginPage() {
  const router = useRouter();
  const [isLoading, setIsLoading] = useState(false);
  const setAuth = useAuthStore((state) => state.setAuth);
  const addToast = useToastStore((state) => state.addToast);

  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<LoginFormData>({
    resolver: zodResolver(loginSchema),
  });

  const onSubmit = async (data: LoginFormData) => {
    setIsLoading(true);
    try {
      const response = await api.login(data.email, data.password);

      // Verify user is an admin
      if (response.user.role !== 'PlatformAdmin') {
        addToast({
          type: 'error',
          title: 'Access Denied',
          description: 'This login is for platform administrators only.',
        });
        setIsLoading(false);
        return;
      }

      setAuth(response.token, response.user);
      addToast({
        type: 'success',
        title: 'Welcome back!',
        description: `Signed in as ${response.user.email}`,
      });
      router.push('/admin/workspaces');
    } catch (error) {
      addToast({
        type: 'error',
        title: 'Login failed',
        description: error instanceof Error ? error.message : 'Invalid credentials',
      });
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Card className="w-full max-w-md mx-4">
      <CardHeader className="space-y-1">
        <div className="flex items-center justify-center gap-2 mb-2">
          <ShieldAlert className="h-8 w-8 text-red-500" />
        </div>
        <CardTitle className="text-2xl font-bold text-center">Admin Portal</CardTitle>
        <CardDescription className="text-center">
          Sign in with your administrator credentials
        </CardDescription>
      </CardHeader>
      <form onSubmit={handleSubmit(onSubmit)}>
        <CardContent className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="email">Email</Label>
            <Input
              id="email"
              type="email"
              placeholder="admin@example.com"
              autoComplete="email"
              error={!!errors.email}
              {...register('email')}
            />
            {errors.email && (
              <p className="text-sm text-destructive">{errors.email.message}</p>
            )}
          </div>
          <div className="space-y-2">
            <Label htmlFor="password">Password</Label>
            <Input
              id="password"
              type="password"
              placeholder="Enter your password"
              autoComplete="current-password"
              error={!!errors.password}
              {...register('password')}
            />
            {errors.password && (
              <p className="text-sm text-destructive">{errors.password.message}</p>
            )}
          </div>
        </CardContent>
        <CardFooter className="flex flex-col space-y-4">
          <Button type="submit" className="w-full" disabled={isLoading}>
            {isLoading ? 'Signing in...' : 'Sign in to Admin Portal'}
          </Button>
          <p className="text-sm text-muted-foreground text-center">
            Platform administrators only. Workspace users should use the{' '}
            <a href="/login" className="text-primary hover:underline">
              regular login
            </a>.
          </p>
        </CardFooter>
      </form>
    </Card>
  );
}
