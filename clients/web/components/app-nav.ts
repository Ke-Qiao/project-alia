import { ActivityIcon, CircleUserIcon } from 'lucide-react';

/** Primary destinations, shared by the desktop sidebar and the mobile bottom bar. */
export const APP_NAV = [
  { href: '/', label: '概览', icon: ActivityIcon },
  { href: '/account', label: '账号', icon: CircleUserIcon }
] as const;
