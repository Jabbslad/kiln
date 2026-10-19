import { z } from 'zod'

export const boxStates = [
  'running',
  'paused',
  'stopped',
  'failed',
  'creating',
  'restoring',
  'unknown',
] as const
const identifier = z.string().min(1).max(160)
const measurement = z.number().finite().nonnegative().nullable()
const resources = z.object({
  vcpus: z.number().int().positive(),
  memory_bytes: z.number().int().positive(),
  disk_bytes: z.number().int().positive(),
})
const metrics = z.object({
  cpu_percent: z.number().min(0).max(100).nullable(),
  memory_bytes: measurement,
  disk_bytes: measurement,
})
const timing = z.object({
  method: z.string(),
  cache: z.string(),
  disk_copy: z.string(),
  storage_ms: measurement,
  vmm_ms: measurement,
  restore_ms: measurement,
  agent_ms: measurement,
  init_ms: measurement,
  first_command_ms: measurement,
  total_ms: measurement,
})

export const dashboardSchema = z.object({
  schema_version: z.literal(1),
  observed_at: z.iso.datetime(),
  workspace: z.string().min(1),
  host: z.object({ name: z.string(), state: z.enum(['online', 'offline', 'unknown']), metrics }),
  boxes: z.array(
    z.object({
      id: identifier,
      name: z.string().min(1),
      state: z.enum(boxStates),
      image: z.string(),
      resources: resources.nullable(),
      metrics,
      created_at: z.iso.datetime(),
      last_operation: z.string().nullable(),
      last_error: z.string().nullable(),
      restored_from: z.string().nullable(),
      timing: timing.nullable(),
    }),
  ),
  templates: z.array(
    z.object({
      id: identifier,
      name: z.string(),
      version: z.string(),
      description: z.string(),
      compatible: z.boolean(),
      development_only: z.boolean(),
      resources,
    }),
  ),
  checkpoints: z.array(
    z.object({
      id: identifier,
      box_id: identifier,
      name: z.string(),
      created_at: z.iso.datetime(),
      size_bytes: measurement,
      compatible: z.boolean(),
    }),
  ),
  operations: z.array(
    z.object({
      id: identifier,
      box_id: identifier,
      box_name: z.string(),
      kind: z.string(),
      state: z.enum(['queued', 'running', 'completed', 'failed']),
      created_at: z.iso.datetime(),
      detail: z.string(),
      steps: z.array(
        z.object({
          label: z.string(),
          state: z.enum(['pending', 'running', 'completed', 'failed']),
        }),
      ),
    }),
  ),
})

export type Dashboard = z.infer<typeof dashboardSchema>
export type Box = Dashboard['boxes'][number]
export type Template = Dashboard['templates'][number]
export type Checkpoint = Dashboard['checkpoints'][number]
export type Operation = Dashboard['operations'][number]
export type BoxState = Box['state']

export async function fetchDashboard(
  signal?: AbortSignal,
  fetcher: typeof fetch = fetch,
): Promise<Dashboard> {
  const response = await fetcher('/api/v1/dashboard', {
    signal,
    credentials: 'same-origin',
    cache: 'no-store',
    headers: { Accept: 'application/json' },
  })
  if (!response.ok)
    throw new Error(
      `The dashboard API returned HTTP ${response.status}. No box state has been inferred.`,
    )
  if (!response.headers.get('content-type')?.includes('application/json')) {
    throw new Error('The dashboard API is not available at /api/v1/dashboard.')
  }
  const result = dashboardSchema.safeParse(await response.json())
  if (!result.success)
    throw new Error(
      'The dashboard API returned an incompatible response. Expected schema version 1 with observed resource state.',
    )
  return result.data
}

export function filterBoxes(boxes: Box[], search: string, state: BoxState | 'all') {
  const needle = search.trim().toLowerCase()
  return boxes.filter(
    (box) =>
      (state === 'all' || box.state === state) &&
      `${box.name} ${box.id}`.toLowerCase().includes(needle),
  )
}

export function formatMetric(value: number | null, suffix = '') {
  return value === null ? '—' : `${Number(value.toFixed(1))}${suffix}`
}

export function formatBytes(bytes: number | null) {
  if (bytes === null) return '—'
  if (bytes === 0) return '0 B'
  const power = Math.min(4, Math.floor(Math.log(bytes) / Math.log(1024)))
  return `${Number((bytes / 1024 ** power).toFixed(1))} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][power]}`
}

export function timestamp(value: string) {
  return (
    new Intl.DateTimeFormat('en-GB', {
      dateStyle: 'medium',
      timeStyle: 'short',
      timeZone: 'UTC',
    }).format(new Date(value)) + ' UTC'
  )
}
