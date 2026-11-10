import { HealthCard } from '@/components/health-card';
import { PageHeading } from '@/components/page-heading';

export default function OverviewPage() {
  return (
    <>
      <PageHeading description="Alia 服务及其各模块当前是否正常。" title="概览" />
      <HealthCard />
    </>
  );
}
