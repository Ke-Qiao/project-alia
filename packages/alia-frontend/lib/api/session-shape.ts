import type { SessionUser } from '@/lib/session/provider';

export type SessionContextShape = {
  effectiveBaseUrl: string | null;
  accessToken: string | null;
  refreshToken: string | null;
  user: SessionUser | null;
};
