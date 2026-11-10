/**
 * Error thrown by every request made through the app's API client.
 *
 * `status` is the HTTP status, or 0 when no HTTP response was received
 * (server down, DNS failure, blocked cross-origin request).
 */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string | undefined;

  constructor(status: number, message: string, code?: string, options?: ErrorOptions) {
    super(message, options);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

export const NETWORK_ERROR = 'NETWORK_ERROR';
export const INVALID_RESPONSE = 'INVALID_RESPONSE';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

/**
 * Normalizes whatever the generated client throws. The server's error body,
 * `{ error: { code, message } }`, is not declared in OpenAPI, so it is read
 * defensively instead of being asserted as a type.
 */
export function toApiError(error: unknown, response: Pick<Response, 'ok' | 'status' | 'statusText'> | undefined): ApiError {
  if (error instanceof ApiError) return error;

  if (!response) {
    const message = isRecord(error) && typeof error.message === 'string' ? error.message : 'network error';
    return new ApiError(0, message, NETWORK_ERROR, { cause: error });
  }

  // A 2xx response that failed to parse is not an Alia API response.
  if (response.ok) {
    return new ApiError(response.status, 'unexpected response body', INVALID_RESPONSE);
  }

  if (isRecord(error) && isRecord(error.error)) {
    const { code, message } = error.error;
    return new ApiError(
      response.status,
      typeof message === 'string' && message ? message : response.statusText,
      typeof code === 'string' ? code : undefined
    );
  }

  return new ApiError(response.status, typeof error === 'string' && error ? error : response.statusText);
}

/** User-facing text for errors that have no more specific handling at the call site. */
export function describeApiError(error: unknown, apiUrl: string): string {
  if (!(error instanceof ApiError)) return '发生了未知错误，请重试。';
  if (error.status === 0) return `无法连接到 Alia 服务（${apiUrl}）。请确认服务已启动且网络可达。`;
  if (error.code === INVALID_RESPONSE) return `${apiUrl} 返回了无法识别的响应，请检查 API 地址是否指向 Alia 服务。`;
  if (error.status === 401) return '登录状态无效，请重新登录。';
  if (error.status === 403) return '当前账号没有权限执行此操作。';
  if (error.status === 429) return '请求过于频繁，请稍后再试。';
  if (error.status >= 500) return `服务暂时不可用（${error.status}），请稍后重试。`;
  return error.message || `请求失败（${error.status}）。`;
}

/** Network failures and 5xx may recover on their own; 4xx will not. */
export function isRetryableError(error: unknown): boolean {
  return !(error instanceof ApiError) || error.status === 0 || error.status >= 500;
}
