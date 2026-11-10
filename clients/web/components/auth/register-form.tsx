'use client';

import { CircleAlertIcon } from 'lucide-react';
import Link from 'next/link';
import { useRef, useState } from 'react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardDescription, CardFooter, CardHeader, CardPanel, CardTitle } from '@/components/ui/card';
import { Field, FieldDescription, FieldError, FieldLabel } from '@/components/ui/field';
import { Form } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ApiError, describeApiError } from '@/lib/api-error';
import { aliaApiUrl } from '@/lib/api-url';
import { register } from '@/lib/api/generated';
import { sessionStore } from '@/lib/auth/session';
import { useMutation } from '@/lib/data';

/** Maps the server's validation messages (packages/alia-server/src/api/auth.rs) to the field they concern. */
function validationErrors(message: string): Record<string, string> | null {
  if (message.includes('already registered')) return { email: '该邮箱已注册，请直接登录。' };
  if (message.includes('email')) return { email: '请输入有效的邮箱地址。' };
  if (message.includes('name')) return { name: '名称不能为空，且不能过长。' };
  if (message.includes('password')) return { password: '密码至少需要 8 个字符。' };
  return null;
}

export function RegisterForm() {
  const { trigger, isMutating } = useMutation(register);
  const submittingRef = useRef(false);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [formError, setFormError] = useState<string | null>(null);

  async function handleSubmit(values: Record<string, unknown>) {
    if (submittingRef.current) return;
    submittingRef.current = true;
    setFieldErrors({});
    setFormError(null);

    try {
      const data = await trigger({
        body: {
          email: String(values.email).trim(),
          name: String(values.name).trim(),
          password: String(values.password)
        }
      });
      // RequireGuest navigates once the session is stored.
      sessionStore.set({ accessToken: data.access_token, refreshToken: data.refresh_token, user: data.user });
    } catch (error) {
      const errors = error instanceof ApiError && error.status === 400 ? validationErrors(error.message) : null;
      if (errors) setFieldErrors(errors);
      else setFormError(describeApiError(error, aliaApiUrl));
    } finally {
      submittingRef.current = false;
    }
  }

  return (
    <Card className="w-full max-w-sm">
      <CardHeader>
        <CardTitle render={<h1 />}>注册 Alia 账号</CardTitle>
        <CardDescription>此服务上的第一个账号将成为管理员。</CardDescription>
      </CardHeader>
      <CardPanel>
        <Form className="flex flex-col gap-4" errors={fieldErrors} onFormSubmit={handleSubmit}>
          {formError && (
            <Alert variant="error">
              <CircleAlertIcon />
              <AlertTitle>注册失败</AlertTitle>
              <AlertDescription>{formError}</AlertDescription>
            </Alert>
          )}
          <Field name="name">
            <FieldLabel>名称</FieldLabel>
            <Input autoComplete="name" required />
            <FieldError />
          </Field>
          <Field name="email">
            <FieldLabel>邮箱</FieldLabel>
            <Input autoComplete="email" required type="email" />
            <FieldError />
          </Field>
          <Field name="password">
            <FieldLabel>密码</FieldLabel>
            <Input autoComplete="new-password" minLength={8} required type="password" />
            <FieldDescription>至少 8 个字符。</FieldDescription>
            <FieldError />
          </Field>
          <Button className="w-full" loading={isMutating} type="submit">注册</Button>
        </Form>
      </CardPanel>
      <CardFooter className="justify-center gap-1 text-muted-foreground text-sm">
        已有账号？
        <Button render={<Link href="/login" />} variant="link">登录</Button>
      </CardFooter>
    </Card>
  );
}
