import { AppShell } from '@/components/app-shell';
import { RequireSession } from '@/components/auth/session-guard';

export default function AppLayout({ children }: React.PropsWithChildren) {
  return (
    <RequireSession>
      <AppShell>{children}</AppShell>
    </RequireSession>
  );
}
