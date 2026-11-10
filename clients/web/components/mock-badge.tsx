import { Badge } from '@/components/ui/badge';
import { mockEnabled } from '@/lib/mock';

/** Marks every screen while mock mode is on, so mock data is never mistaken for a real server. */
export function MockBadge() {
  if (!mockEnabled) return null;
  return <Badge title="NEXT_PUBLIC_ALIA_MOCK=1：数据来自浏览器内的模拟服务" variant="warning">Mock 数据</Badge>;
}
