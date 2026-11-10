'use client';

import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardFooter, CardHeader, CardPanel, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { RequestError } from '@/components/request-error';
import { getMe, logout } from '@/lib/api/generated';
import { sessionStore } from '@/lib/auth/session';
import { signOut } from '@/lib/auth/sign-out';
import { useData, useMutation } from '@/lib/data';

// Values accepted by packages/alia-server/src/api/users.rs.
const ROLE_LABELS = new Map([['admin', '管理员'], ['user', '成员']]);
const STATUS_LABELS = new Map([['active', '正常'], ['disabled', '已停用'], ['deleted', '已删除']]);

function formatTime(value: string | null | undefined): string {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN');
}

function Row({ label, children }: React.PropsWithChildren<{ label: string }>) {
  return (
    <div className="flex items-center justify-between gap-4 py-2">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </div>
  );
}

export function AccountCard() {
  const { trigger: revokeRefreshToken } = useMutation(logout);
  const { data, error, isValidating, mutate } = useData(getMe, {});
  const failed = error !== undefined;

  return (
    <Card>
      <CardHeader>
        <CardTitle render={<h2 />}>账号信息</CardTitle>
      </CardHeader>
      <CardPanel>
        {failed && !data && (
          <RequestError error={error} onRetry={() => { void mutate(); }} retrying={isValidating} title="无法加载账号信息" />
        )}
        {!failed && !data && <Skeleton className="h-40" />}
        {data && (
          <dl className="divide-y text-sm">
            <Row label="名称">{data.name}</Row>
            <Row label="邮箱">{data.email}</Row>
            <Row label="角色"><Badge variant="outline">{ROLE_LABELS.get(data.role) ?? data.role}</Badge></Row>
            <Row label="状态">
              <Badge variant={data.status === 'active' ? 'success' : 'warning'}>{STATUS_LABELS.get(data.status) ?? data.status}</Badge>
            </Row>
            <Row label="注册时间">{formatTime(data.created_at)}</Row>
            <Row label="上次登录">{formatTime(data.last_login_at)}</Row>
          </dl>
        )}
      </CardPanel>
      <CardFooter className="justify-between gap-4 border-t">
        <p className="text-muted-foreground text-sm">退出后需要重新登录才能访问。</p>
        <Button onClick={() => { void signOut(sessionStore, refreshToken => revokeRefreshToken({ body: { refresh_token: refreshToken } })); }} variant="outline">退出登录</Button>
      </CardFooter>
    </Card>
  );
}
