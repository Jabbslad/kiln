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

export function requestId() {
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

export type OperationReceipt = {
  id: string
  box_id: string
  state: string
  result?: {
    type?: string
    id?: string
    box_id?: string
    state?: string
    stdout?: number[]
    stderr?: number[]
    exit_code?: number | null
    truncated?: boolean
    timed_out?: boolean
  } | null
  error?: { message?: string } | null
}

export type OperationTrackKind =
  'create' | 'lifecycle' | 'checkpoint' | 'restore' | 'delete' | 'exec'

const OPERATION_POLL_START_MS = 50
const OPERATION_POLL_MAX_MS = 2_000
const OPERATION_POLL_GROWTH = 1.5

const OPERATION_WAIT_MS: Record<OperationTrackKind, number> = {
  create: 300_000,
  lifecycle: 120_000,
  checkpoint: 180_000,
  restore: 300_000,
  delete: 120_000,
  exec: 60_000,
}

function operationTrackKind(action: OperationAction): OperationTrackKind {
  switch (action.type) {
    case 'create':
      return 'create'
    case 'pause':
    case 'resume':
    case 'stop':
    case 'start':
      return 'lifecycle'
    case 'checkpoint':
      return 'checkpoint'
    case 'restore':
      return 'restore'
    case 'delete':
      return 'delete'
    case 'exec':
      return 'exec'
  }
}

function operationWaitMs(action: OperationAction) {
  if (action.type === 'exec') return action.request.timeout_ms + 30_000
  return OPERATION_WAIT_MS[operationTrackKind(action)]
}

export function operationPendingLabel(action: OperationAction) {
  switch (action.type) {
    case 'create':
      return 'Creating…'
    case 'pause':
      return 'Pausing…'
    case 'resume':
      return 'Resuming…'
    case 'stop':
      return 'Stopping…'
    case 'start':
      return 'Starting…'
    case 'checkpoint':
      return 'Creating checkpoint…'
    case 'restore':
      return 'Restoring…'
    case 'delete':
      return 'Deleting…'
    case 'exec':
      return 'Running…'
  }
}

function operationInProgressMessage(action: OperationAction) {
  switch (operationTrackKind(action)) {
    case 'create':
      return 'Create is still in progress. Refresh the dashboard to see the box once the control plane records it.'
    case 'lifecycle':
      return 'This box change is still in progress. Refresh the dashboard to see updated state.'
    case 'checkpoint':
      return 'Checkpoint creation is still in progress. Refresh the dashboard to see it once recorded.'
    case 'restore':
      return 'Restore is still in progress. Refresh the dashboard to see updated state.'
    case 'delete':
      return 'Delete is still in progress. Refresh the dashboard to confirm removal.'
    case 'exec':
      return 'The command is still running. Refresh or retry later if output does not appear.'
  }
}

function operationFailureFallback(action: OperationAction) {
  switch (operationTrackKind(action)) {
    case 'create':
      return 'Create did not succeed. The box is shown only after the control plane records it.'
    default:
      return 'The operation did not succeed. Inventory updates only after the control plane records them.'
  }
}

function settleOperationReceipt(receipt: OperationReceipt, action: OperationAction) {
  if (receipt.state === 'succeeded') return receipt
  if (receipt.state === 'failed') {
    throw new Error(receipt.error?.message || operationFailureFallback(action))
  }
  if (receipt.state === 'running' || receipt.state === 'unknown') {
    throw new Error(operationInProgressMessage(action))
  }
  throw new Error(receipt.error?.message || operationFailureFallback(action))
}

export type OperationAction =
  | { type: 'create'; template: string; name: string }
  | { type: 'pause'; id: string }
  | { type: 'resume'; id: string }
  | { type: 'stop'; id: string; force: boolean }
  | { type: 'start'; id: string }
  | { type: 'delete'; id: string }
  | { type: 'exec'; id: string; request: { argv: string[]; timeout_ms: number; cwd?: string } }
  | { type: 'checkpoint'; id: string }
  | {
      type: 'restore'
      id: string
      checkpoint: string
      acknowledge_external_state_replay: true
    }

async function rejected(response: Response) {
  let code = ''
  try {
    code = ((await response.json()) as { code?: string }).code ?? ''
  } catch {
    code = ''
  }
  if (response.status === 401 || code === 'unauthorized') {
    return new Error('Unauthenticated (HTTP 401). No operation was accepted.')
  }
  if (code === 'invalid_origin') {
    return new Error('The origin was rejected. No operation was accepted.')
  }
  if (code === 'invalid_csrf' || response.status === 403) {
    return new Error('The CSRF check was rejected. No operation was accepted.')
  }
  return new Error(
    `The operation was not accepted (HTTP ${response.status}). No change was recorded.`,
  )
}

function pause(ms: number) {
  return new Promise<void>((resolve) => {
    setTimeout(resolve, ms)
  })
}

export async function trackOperation(
  action: OperationAction,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
  id = requestId(),
  waitMs?: number,
): Promise<OperationReceipt> {
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
  if (response.status !== 200 && response.status !== 202) throw await rejected(response)
  let receipt = (await response.json()) as OperationReceipt
  let interval = OPERATION_POLL_START_MS
  const deadline = Date.now() + (waitMs ?? operationWaitMs(action))
  while (receipt.state === 'running' && Date.now() < deadline) {
    await pause(interval)
    interval = Math.min(OPERATION_POLL_MAX_MS, Math.round(interval * OPERATION_POLL_GROWTH))
    const polled = await fetcher(`/v1/operations/${receipt.id}`, {
      credentials: 'same-origin',
      cache: 'no-store',
      headers: { Accept: 'application/json' },
    })
    if (!polled.ok) {
      if (polled.status === 401 || polled.status === 403) throw await rejected(polled)
      throw new Error('unknown')
    }
    receipt = (await polled.json()) as OperationReceipt
  }
  return settleOperationReceipt(receipt, action)
}

/** @deprecated Use trackOperation */
export const submitOperation = trackOperation

async function awaitDashboard(
  fetcher: typeof fetch,
  seen: (dashboard: Dashboard) => boolean,
  pending: string,
) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    const dashboard = await fetchDashboard(undefined, fetcher)
    if (seen(dashboard)) return dashboard
  }
  throw new Error(pending)
}

export async function createBox(
  template: string,
  name: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const action = { type: 'create', template, name } as const
  const receipt = await trackOperation(action, fetcher, cookie)
  return awaitDashboard(
    fetcher,
    (dashboard) => dashboard.boxes.some((box) => box.id === receipt.box_id),
    'The control plane has not observed the new box.',
  )
}

export async function createCheckpoint(
  boxId: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
  id = requestId(),
): Promise<Dashboard> {
  const receipt = await trackOperation({ type: 'checkpoint', id: boxId }, fetcher, cookie, id)
  const checkpointId = receipt.result?.type === 'checkpoint' ? receipt.result.id : undefined
  if (!checkpointId || (receipt.result?.box_id && receipt.result.box_id !== boxId)) {
    throw new Error('unknown')
  }
  return awaitDashboard(
    fetcher,
    (dashboard) =>
      dashboard.checkpoints.some(
        (checkpoint) => checkpoint.id === checkpointId && checkpoint.box_id === boxId,
      ),
    'The control plane has not observed the checkpoint.',
  )
}

export async function restoreCheckpoint(
  boxId: string,
  checkpointId: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
  id = requestId(),
): Promise<Dashboard> {
  const receipt = await trackOperation(
    {
      type: 'restore',
      id: boxId,
      checkpoint: checkpointId,
      acknowledge_external_state_replay: true,
    },
    fetcher,
    cookie,
    id,
  )
  if (receipt.box_id !== boxId) throw new Error('unknown')
  return awaitDashboard(
    fetcher,
    (dashboard) =>
      dashboard.boxes.some((box) => box.id === boxId && box.restored_from === checkpointId),
    'The control plane has not observed the restore.',
  )
}

export async function applyLifecycle(
  action: 'pause' | 'resume' | 'stop' | 'start',
  boxId: string,
  force = false,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const operation =
    action === 'stop'
      ? ({ type: 'stop', id: boxId, force } as const)
      : ({ type: action, id: boxId } as const)
  const receipt = await trackOperation(operation, fetcher, cookie)
  const observed = receipt.result?.state
  if (!observed) throw new Error('The control plane has not observed the box state.')
  return awaitDashboard(
    fetcher,
    (dashboard) =>
      dashboard.boxes.some((box) => box.id === receipt.box_id && box.state === observed),
    'The control plane has not observed the box state.',
  )
}

export type ExecOutput = {
  exitCode: number | null
  stdout: string
  stderr: string
  truncated: boolean
  timedOut: boolean
}

export async function execCommand(
  boxId: string,
  argv: string[],
  timeoutMs: number,
  cwd?: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<ExecOutput> {
  const request: { argv: string[]; timeout_ms: number; cwd?: string } = {
    argv,
    timeout_ms: timeoutMs,
  }
  if (cwd) request.cwd = cwd
  const receipt = await trackOperation({ type: 'exec', id: boxId, request }, fetcher, cookie)
  const stdout = receipt.result?.stdout
  const stderr = receipt.result?.stderr
  if (receipt.result?.type !== 'exec' || !stdout || !stderr) throw new Error('unknown')
  return {
    exitCode: receipt.result.exit_code ?? null,
    stdout: new TextDecoder().decode(Uint8Array.from(stdout)),
    stderr: new TextDecoder().decode(Uint8Array.from(stderr)),
    truncated: receipt.result.truncated === true,
    timedOut: receipt.result.timed_out === true,
  }
}

export async function deleteBox(
  boxId: string,
  fetcher: typeof fetch = fetch,
  cookie = globalThis.document?.cookie ?? '',
): Promise<Dashboard> {
  const receipt = await trackOperation({ type: 'delete', id: boxId }, fetcher, cookie)
  return awaitDashboard(
    fetcher,
    (dashboard) => !dashboard.boxes.some((box) => box.id === receipt.box_id),
    'The control plane has not observed the deletion.',
  )
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
