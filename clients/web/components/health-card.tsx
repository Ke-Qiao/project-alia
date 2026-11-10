'use client';

import { Badge } from '@/components/ui/badge';
import { Card, CardAction, CardDescription, CardHeader, CardPanel, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { RequestError } from '@/components/request-error';
import { healthCheck } from '@/lib/api/generated';
import { useData } from '@/lib/data';

type BadgeVariant = React.ComponentProps<typeof Badge>['variant'];

// Values produced by packages/alia-server/src/api/health.rs.
const STATUS = new Map<string, { label: string, variant: BadgeVariant }>([
  ['ok', { label: '正常', variant: 'success' }],
  ['healthy', { label: '正常', variant: 'success' }],
  ['degraded', { label: '降级', variant: 'warning' }],
  ['unhealthy', { label: '异常', variant: 'error' }]
]);

const REFRESH_INTERVAL_MS = 30 * 1000;
const moduleNames = new Intl.Collator('en');

function StatusBadge({ status }: { status: string }) {
  const known = STATUS.get(status);
  return <Badge variant={known?.variant ?? 'outline'}>{known?.label ?? status}</Badge>;
}

export function HealthCard() {
  const { data, error, isValidating, mutate } = useData(healthCheck, {}, { refreshInterval: REFRESH_INTERVAL_MS });
  const failed = error !== undefined;
  const modules = data ? Object.entries(data.modules).sort(([a], [b]) => moduleNames.compare(a, b)) : [];

  return (
    <Card>
      <CardHeader>
        <CardTitle render={<h2 />}>服务状态</CardTitle>
        <CardDescription>{data ? `alia-server ${data.version}` : 'alia-server'}</CardDescription>
        {data && !failed && <CardAction><StatusBadge status={data.status} /></CardAction>}
      </CardHeader>
      <CardPanel className="flex flex-col gap-4">
        {failed && (
          <RequestError
            error={error}
            onRetry={() => { void mutate(); }}
            retrying={isValidating}
            title={data ? '无法刷新服务状态，以下为上次结果' : '无法获取服务状态'}
          />
        )}
        {!failed && !data && <Skeleton className="h-40" />}
        {data && modules.length === 0 && <p className="text-muted-foreground text-sm">服务没有上报任何模块。</p>}
        {data && modules.length > 0 && (
          <ul className="divide-y text-sm">
            {modules.map(([name, module]) => (
              <li className="flex flex-col gap-1 py-2" key={name}>
                <div className="flex items-center justify-between gap-4">
                  <span className="truncate font-mono">{name}</span>
                  <StatusBadge status={module.status} />
                </div>
                {module.message && <p className="break-words text-muted-foreground text-xs">{module.message}</p>}
              </li>
            ))}
          </ul>
        )}
      </CardPanel>
    </Card>
  );
}
