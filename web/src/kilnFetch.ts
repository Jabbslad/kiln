export const KILN_UNREACHABLE_MESSAGE = "Couldn't reach the server"

export class KilnUnreachableError extends Error {
  readonly unreachable = true

  constructor() {
    super(KILN_UNREACHABLE_MESSAGE)
    this.name = 'KilnUnreachableError'
  }
}

export function isUnreachableError(error: unknown): boolean {
  return error instanceof KilnUnreachableError
}

export function apiErrorMessage(reason: unknown, fallback: string): string {
  return reason instanceof Error ? reason.message : fallback
}

export async function kilnFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
  fetcher: typeof fetch = fetch,
): Promise<Response> {
  try {
    return await fetcher(input, init)
  } catch (reason) {
    if (reason instanceof KilnUnreachableError) throw reason
    if (reason instanceof DOMException && reason.name === 'AbortError') {
      throw new KilnUnreachableError()
    }
    if (reason instanceof TypeError) {
      throw new KilnUnreachableError()
    }
    throw reason
  }
}
