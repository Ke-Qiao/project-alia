import type { Metadata } from 'next';

import { LoginForm } from '@/components/auth/login-form';

export const metadata: Metadata = { title: '登录 · Alia' };

export default function LoginPage() {
  return <LoginForm />;
}
