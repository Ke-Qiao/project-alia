import type { Metadata } from 'next';

import { AccountCard } from '@/components/account-card';
import { PageHeading } from '@/components/page-heading';

export const metadata: Metadata = { title: '账号 · Alia' };

export default function AccountPage() {
  return (
    <>
      <PageHeading description="当前登录的账号，以及退出登录。" title="账号" />
      <AccountCard />
    </>
  );
}
