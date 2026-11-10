'use client';

import { CircleAlertIcon } from 'lucide-react';

import { Alert, AlertAction, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { describeApiError } from '@/lib/api-error';
import { aliaApiUrl } from '@/lib/api-url';

interface RequestErrorProps {
  title: string,
  error: unknown,
  retrying: boolean,
  onRetry: () => void
}

/** A failed read, shown in place of the data so it cannot be mistaken for an empty result. */
export function RequestError({ title, error, retrying, onRetry }: RequestErrorProps) {
  return (
    <Alert variant="error">
      <CircleAlertIcon />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>{describeApiError(error, aliaApiUrl)}</AlertDescription>
      <AlertAction>
        <Button loading={retrying} onClick={onRetry} size="xs" variant="outline">重试</Button>
      </AlertAction>
    </Alert>
  );
}
