import { afterEach, describe, expect, it, vi } from 'vitest'
import { KILN_UNREACHABLE_MESSAGE, isUnreachableError, kilnFetch } from './kilnFetch'

afterEach(() => vi.unstubAllGlobals())

describe('kilnFetch', () => {
  it('maps fetch network failures to a clear unreachable message', async () => {
    const fetcher = vi.fn().mockRejectedValue(new TypeError('Load failed'))
    await expect(kilnFetch('/api/v1/dashboard', undefined, fetcher)).rejects.toSatisfy(
      (error: unknown) => {
        expect(error).toBeInstanceOf(Error)
        expect((error as Error).message).toBe(KILN_UNREACHABLE_MESSAGE)
        expect(isUnreachableError(error)).toBe(true)
        return true
      },
    )
  })

  it('returns HTTP responses without treating them as network errors', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response('Unavailable', { status: 503 }))
    const response = await kilnFetch('/api/v1/dashboard', undefined, fetcher)
    expect(response.status).toBe(503)
  })
})
