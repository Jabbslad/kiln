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

function requestId() {
  const bytes = new Uint8Array(16)
  crypto.getRandomValues(bytes)
  return [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}

export function readCsrf(cookie: string) {
  const prefix = '__Host-kiln_csrf='
  const value = cookie
    .split(';')
    .map((part) => part.trim())
    .find((part) => part.startsWith(prefix))
  return value ? value.slice(prefix.length) : ''
}

type OperationReceipt = {
  id: string
  box_id: string
  state: string
  result?: { state?: string } | null
  error?: { message?: string } | null
}

type SubmittedAction =
  | { type: 'create'; template: string; name: string }
  | { type: 'pause'; id: string }
  | { type: 'resume'; id: string }
  | { type: 'stop'; id: string; force: boolean }
  | { type: 'start'; id: string }
  | { type: 'delete'; id: string }

async function submitOperation(
  action: SubmittedAction,
  fetcher: typeof fetch,
  cookie: string,
  rejected: (status: number) => string,
  stalled: (status: number) => string,
): Promise<OperationReceipt> {
  const id = requestId()
  const response = await fetcher('/v1/operations', {
    method: 'POST',
    credentials: 'same-origin',
    cache: 'no-store',
    headers: {
      Accept: 'application/json',
      'Content-Type': 'application/json',
      'x-kiln-csrf': readCsrf(cookie),
    },
    body: JSON.stringify({ id, action }),
  })
  if (response.status !== 200 && response.status !== 202) throw new Error(rejected(response.status))
  let receipt = (await response.json()) as OperationReceipt
  for (let attempt = 0; attempt < 20 && receipt.state === 'running'; attempt += 1) {
    const polled = await fetcher(`/v1/operations/${receipt.id}`, {
      credentials: 'same-origin',
      cache: 'no-store',
      headers: { Accept: 'application/json' },
    })
    if (!polled.ok) throw new Error(stalled(polled.status))
    receipt = (await polled.json()) as OperationReceipt
  }
  return receipt
}

export async function createBox(
  template: string,
  name: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const receipt = await submitOperation(
    { type: 'create', template, name },
    fetcher,
    cookie,
    (status) => `Create was not accepted (HTTP ${status}). No box was added.`,
    (status) => `Operation status returned HTTP ${status}. The box is unchanged.`,
  )
  if (receipt.state !== 'succeeded') {
    throw new Error(
      receipt.error?.message ||
        'Create did not succeed. The box is shown only after the control plane records it.',
    )
  }
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const dashboard = await fetchDashboard(undefined, fetcher)
    if (dashboard.boxes.some((box) => box.id === receipt.box_id)) return dashboard
  }
  throw new Error('The control plane has not observed the new box.')
}

export async function applyLifecycle(
  action: 'pause' | 'resume' | 'stop' | 'start',
  boxId: string,
  force = false,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const receipt = await submitOperation(
    action === 'stop' ? { type: 'stop', id: boxId, force } : { type: action, id: boxId },
    fetcher,
    cookie,
    (status) => `The operation was not accepted (HTTP ${status}). The box is unchanged.`,
    (status) => `Operation status returned HTTP ${status}. The box is unchanged.`,
  )
  if (receipt.state === 'unknown') throw new Error('The outcome is unknown.')
  if (receipt.state !== 'succeeded') {
    throw new Error(
      receipt.error?.message ||
        'The operation did not succeed. The box state is shown only after the control plane reports it.',
    )
  }
  const observed = receipt.result?.state
  if (!observed) throw new Error('The control plane has not observed the box state.')
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const dashboard = await fetchDashboard(undefined, fetcher)
    if (dashboard.boxes.some((box) => box.id === receipt.box_id && box.state === observed)) {
      return dashboard
    }
  }
  throw new Error('The control plane has not observed the box state.')
}

export async function deleteBox(
  boxId: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const receipt = await submitOperation(
    { type: 'delete', id: boxId },
    fetcher,
    cookie,
    (status) => `The operation was not accepted (HTTP ${status}). The box is unchanged.`,
    (status) => `Operation status returned HTTP ${status}. The box is unchanged.`,
  )
  if (receipt.state === 'unknown') throw new Error('The outcome is unknown.')
  if (receipt.state !== 'succeeded') {
    throw new Error(
      receipt.error?.message ||
        'The operation did not succeed. The box stays until the control plane observes it gone.',
    )
  }
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const dashboard = await fetchDashboard(undefined, fetcher)
    if (!dashboard.boxes.some((box) => box.id === receipt.box_id)) return dashboard
  }
  throw new Error('The control plane has not observed the deletion.')
}

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
