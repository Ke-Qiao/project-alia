import { t } from '@/lib/i18n';
import { useSession } from '@/lib/session/provider';
import * as React from 'react';

export function useTranslation() {
  const { locale } = useSession();
  return React.useCallback((key: string) => t(locale, key), [locale]);
}
