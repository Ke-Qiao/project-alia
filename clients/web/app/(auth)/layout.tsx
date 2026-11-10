import { MockBadge } from '@/components/mock-badge';
import { RequireGuest } from '@/components/auth/session-guard';

export default function AuthLayout({ children }: React.PropsWithChildren) {
  return (
    <RequireGuest>
      <main className="flex flex-1 flex-col items-center justify-center gap-4 px-4 py-12">
        <MockBadge />
        {children}
      </main>
    </RequireGuest>
  );
}
