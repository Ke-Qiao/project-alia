import type { Metadata } from 'next';

import { DataFetchingProvider } from '@/lib/data-provider';

import './globals.css';

export const metadata: Metadata = {
  title: 'Alia'
};

export default function RootLayout({ children }: React.PropsWithChildren) {
  return (
    <html lang="zh-CN">
      <body className="relative font-sans">
        <div className="isolate relative flex min-h-svh flex-col">
          <DataFetchingProvider>{children}</DataFetchingProvider>
        </div>
      </body>
    </html>
  );
}
