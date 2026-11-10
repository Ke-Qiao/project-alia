'use client';

import { useRouter } from 'next/navigation';
import { useEffect } from 'react';
import { useSWRConfig } from 'swr';
import { isTayoriKey } from 'tayori';

import { Spinner } from '@/components/ui/spinner';
import { useSession } from '@/lib/auth/session';

const pendingScreen = (
  <div className="flex flex-1 items-center justify-center">
    <Spinner />
  </div>
);

/** Renders children only for a signed-in user; otherwise sends the browser to /login. */
export function RequireSession({ children }: React.PropsWithChildren) {
  const session = useSession();
  const router = useRouter();
  const { mutate } = useSWRConfig();

  useEffect(() => {
    if (session !== null) return;
    // Cached responses belong to the account that just ended.
    void mutate(isTayoriKey, undefined, { revalidate: false });
    router.replace('/login');
  }, [session, router, mutate]);

  if (!session) return pendingScreen;
  return children;
}

/** Renders children only for a signed-out visitor; a signed-in user goes to the overview. */
export function RequireGuest({ children }: React.PropsWithChildren) {
  const session = useSession();
  const router = useRouter();

  useEffect(() => {
    if (session) router.replace('/');
  }, [session, router]);

  if (session !== null) return pendingScreen;
  return children;
}
