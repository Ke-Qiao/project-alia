import type { HealthResponse, UserResponse } from '@/lib/api/generated/types.gen';

export interface MockAccount {
  user: UserResponse,
  /** Mock-only sign-in password. These accounts do not exist on any server. */
  password: string
}

export interface MockDataset {
  accounts: MockAccount[],
  health: HealthResponse
}

/**
 * Seed data for the in-browser mock host. Shapes come from the generated
 * OpenAPI types; module names and status values follow what
 * packages/alia-server/src/api/health.rs can return.
 */
export const mockDataset: MockDataset = {
  accounts: [
    {
      password: 'alia-mock-admin',
      user: {
        id: '0b6f3c1e-5a52-4c8e-9a54-1f0d6a7e0001',
        email: 'admin@alia.test',
        name: 'Mock 管理员',
        role: 'admin',
        status: 'active',
        created_at: '2026-09-07T02:15:00Z',
        updated_at: '2026-11-10T12:00:00Z',
        last_login_at: '2026-11-10T12:00:00Z'
      }
    },
    {
      password: 'alia-mock-member',
      user: {
        id: '0b6f3c1e-5a52-4c8e-9a54-1f0d6a7e0002',
        email: 'member@alia.test',
        name: 'Mock 成员',
        role: 'user',
        status: 'active',
        created_at: '2026-10-01T08:30:00Z',
        updated_at: '2026-10-01T08:30:00Z',
        last_login_at: null
      }
    },
    {
      password: 'alia-mock-disabled',
      user: {
        id: '0b6f3c1e-5a52-4c8e-9a54-1f0d6a7e0003',
        email: 'disabled@alia.test',
        name: 'Mock 停用账号',
        role: 'user',
        status: 'disabled',
        created_at: '2026-10-12T08:30:00Z',
        updated_at: '2026-10-20T08:30:00Z',
        last_login_at: '2026-10-19T21:04:00Z'
      }
    }
  ],
  health: {
    status: 'degraded',
    version: '0.0.0-mock',
    modules: {
      companion: { status: 'healthy' },
      delivery: { status: 'healthy' },
      embodiment: { status: 'degraded', message: 'mock: no embedded device connected' },
      execution: { status: 'healthy' },
      interaction: { status: 'healthy' },
      memory: { status: 'healthy' },
      platform: { status: 'healthy' }
    }
  }
};
