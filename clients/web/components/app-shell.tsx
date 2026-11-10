'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

import { APP_NAV } from '@/components/app-nav';
import { MockBadge } from '@/components/mock-badge';
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarTrigger,
  useSidebar
} from '@/components/ui/sidebar';
import { useSession } from '@/lib/auth/session';
import { cn } from '@/lib/utils';

const brand = (
  <div className="flex items-center gap-2">
    <span className="font-heading font-semibold">Alia</span>
    <MockBadge />
  </div>
);

function DesktopSidebar({ pathname }: { pathname: string }) {
  const session = useSession();

  return (
    <Sidebar>
      <SidebarHeader className="h-12 justify-center px-4">
        {brand}
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarMenu>
            {APP_NAV.map(({ href, label, icon: Icon }) => (
              <SidebarMenuItem key={href}>
                <SidebarMenuButton
                  aria-current={pathname === href ? 'page' : undefined}
                  isActive={pathname === href}
                  render={<Link href={href} />}
                >
                  <Icon />
                  <span>{label}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            ))}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter className="px-4 pb-4">
        <p className="truncate font-medium text-sm">{session?.user.name}</p>
        <p className="truncate text-muted-foreground text-xs">{session?.user.email}</p>
      </SidebarFooter>
    </Sidebar>
  );
}

function MobileTabBar({ pathname }: { pathname: string }) {
  return (
    <nav aria-label="主导航" className="sticky bottom-0 z-10 border-t bg-background pb-[env(safe-area-inset-bottom)]">
      <ul className="grid auto-cols-fr grid-flow-col">
        {APP_NAV.map(({ href, label, icon: Icon }) => {
          const active = pathname === href;
          return (
            <li key={href}>
              <Link
                aria-current={active ? 'page' : undefined}
                className={cn(
                  'flex h-14 flex-col items-center justify-center gap-1 text-xs outline-none focus-visible:bg-accent',
                  active ? 'font-medium text-foreground' : 'text-muted-foreground'
                )}
                href={href}
              >
                <Icon aria-hidden className="size-5" />
                {label}
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

function Shell({ children }: React.PropsWithChildren) {
  const pathname = usePathname();
  // The sidebar component and this shell must agree on one breakpoint.
  const { isMobile } = useSidebar();

  return (
    <>
      {!isMobile && <DesktopSidebar pathname={pathname} />}
      <SidebarInset>
        <header className="sticky top-0 z-10 flex h-12 items-center gap-2 border-b bg-background px-4">
          {isMobile ? brand : <SidebarTrigger aria-label="收起或展开侧边栏" className="-ms-1.5" />}
        </header>
        <div className="mx-auto flex w-full max-w-2xl flex-1 flex-col gap-6 px-4 py-6 md:py-8">{children}</div>
        {isMobile && <MobileTabBar pathname={pathname} />}
      </SidebarInset>
    </>
  );
}

/** Signed-in frame: sidebar navigation on desktop, bottom tab bar on small screens. */
export function AppShell({ children }: React.PropsWithChildren) {
  return (
    <SidebarProvider>
      <Shell>{children}</Shell>
    </SidebarProvider>
  );
}
