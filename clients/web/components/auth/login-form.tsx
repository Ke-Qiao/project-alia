'use client';

import { CircleAlertIcon, TriangleAlertIcon } from 'lucide-react';
import Link from 'next/link';
import { useRef, useState } from 'react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardFooter, CardHeader, CardPanel, CardTitle } from '@/components/ui/card';
import { Field, FieldError, FieldLabel } from '@/components/ui/field';
import { Form } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ApiError, describeApiError } from '@/lib/api-error';
import { aliaApiUrl } from '@/lib/api-url';
import { login } from '@/lib/api/generated';
import { sessionStore } from '@/lib/auth/session';
import { useMutation } from '@/lib/data';

export function LoginForm() {
  const { trigger, isMutating } = useMutation(login);
  const submittingRef = useRef(false);
  const sessionExpired = sessionStore.getSignOutReason() === 'expired';
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [formError, setFormError] = useState<string | null>(null);

  async function handleSubmit(values: Record<string, unknown>) {
    if (submittingRef.current) return;
    submittingRef.current = true;
    setFieldErrors({});
    setFormError(null);

    try {
      const data = await trigger({
        body: { email: String(values.email).trim(), password: String(values.password) }
      });
      // RequireGuest navigates once the session is stored.
      sessionStore.set({ accessToken: data.access_token, refreshToken: data.refresh_token, user: data.user });
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        // On this endpoint 401 means rejected credentials, not an expired session.
        setFieldErrors({ password: '邮箱或密码不正确。' });
      } else if (error instanceof ApiError && error.status === 400) {
        setFieldErrors({ email: '请输入有效的邮箱地址。' });
      } else {
        setFormError(describeApiError(error, aliaApiUrl));
      }
    } finally {
      submittingRef.current = false;
    }
  }

  return (
    <Card className="w-full max-w-sm">
      <CardHeader>
        <CardTitle render={<h1 />}>登录 Alia</CardTitle>
        <CardDescription>使用邮箱和密码登录。</CardDescription>
      </CardHeader>
      <CardPanel>
        <Form className="flex flex-col gap-4" errors={fieldErrors} onFormSubmit={handleSubmit}>
          {sessionExpired && !formError && (
            <Alert variant="warning">
              <TriangleAlertIcon />
              <AlertTitle>登录已过期</AlertTitle>
              <AlertDescription>请重新登录以继续。</AlertDescription>
            </Alert>
          )}
          {formError && (
            <Alert variant="error">
              <CircleAlertIcon />
              <AlertTitle>登录失败</AlertTitle>
              <AlertDescription>{formError}</AlertDescription>
            </Alert>
          )}
          <Field name="email">
            <FieldLabel>邮箱</FieldLabel>
            <Input autoComplete="email" required type="email" />
            <FieldError />
          </Field>
          <Field name="password">
            <FieldLabel>密码</FieldLabel>
            <Input autoComplete="current-password" required type="password" />
            <FieldError />
          </Field>
          <Button className="w-full" loading={isMutating} type="submit">登录</Button>
        </Form>
      </CardPanel>
      <CardFooter className="justify-center gap-1 text-muted-foreground text-sm">
        还没有账号？
        <Button render={<Link href="/register" />} variant="link">注册</Button>
      </CardFooter>
    </Card>
  );
}
