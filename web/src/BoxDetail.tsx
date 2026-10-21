import { useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import {
  ArrowLeft,
  ArrowUpRight,
  Check,
  CircleAlert,
  Clock3,
  HardDrive,
  Info,
  Lock,
  Pause,
  Play,
  Plus,
  Square,
  Terminal,
  Trash2,
} from 'lucide-react'
import {
  execCommand,
  operationPendingLabel,
  formatBytes,
  formatMetric,
  timestamp,
  type Box,
  type Checkpoint,
  type ExecOutput,
} from './data'
import { DeleteDialog, LifecycleDialog, RestoreDialog } from './dialogs'
import { OperationList } from './Operations'
import {
  AppLink,
  CapabilitySurface,
  CopyButton,
  Empty,
  ResourceMetrics,
  Status,
  Unavailable,
  type PageProps,
} from './ui'
import { capabilityAvailable } from './capabilities'

const tabs = ['Terminal', 'Exec', 'Previews', 'Checkpoints', 'Metrics', 'Activity']

export default function BoxDetail({ data, preview }: PageProps) {
  const { id } = useParams()
  const [params, setParams] = useSearchParams()
  const requestedTab = params.get('tab') ?? 'terminal'
  const activeTab = tabs.some((tab) => tab.toLowerCase() === requestedTab)
    ? requestedTab
    : 'terminal'
  const [action, setAction] = useState<string | null>(null)
  const [restoring, setRestoring] = useState<Checkpoint | null>(null)
  const box = data.boxes.find((item) => item.id === id)
  if (!box)
    return (
      <Empty
        title="Box not found"
        action={
          <AppLink className="button" to="/boxes">
            <ArrowLeft size={15} />
            Back to boxes
          </AppLink>
        }
      >
        This box is not in the current workspace inventory.
      </Empty>
    )
  const checkpoints = data.checkpoints.filter((checkpoint) => checkpoint.box_id === box.id)
  const authenticated = !preview && data.host.state === 'online'
  return (
    <>
      <div className="breadcrumb">
        <AppLink to="/boxes">Boxes</AppLink>
        <span>/</span>
        <span>{box.name}</span>
      </div>
      <header className="box-heading">
        <div>
          <div className="box-title">
            <h1>{box.name}</h1>
            <Status state={box.state} />
          </div>
          <div className="box-id mono">
            {box.id}
            <CopyButton value={box.id} label="Copy box ID" />
          </div>
          <p>
            {box.image}
            {box.resources && (
              <>
                <span>/</span>
                {box.resources.vcpus} vCPU<span>/</span>
                {formatBytes(box.resources.memory_bytes)} RAM<span>/</span>
                {formatBytes(box.resources.disk_bytes)} disk
              </>
            )}
          </p>
        </div>
        <div className="heading-actions">
          {box.state === 'running' && (
            <button className="button" onClick={() => setAction('Pause')}>
              <Pause size={14} />
              Pause
            </button>
          )}
          {box.state === 'paused' && (
            <button className="button primary" onClick={() => setAction('Resume')}>
              <Play size={14} />
              Resume
            </button>
          )}
          {box.state === 'stopped' && (
            <button className="button primary" onClick={() => setAction('Start')}>
              <Play size={14} />
              Start
            </button>
          )}
          {['running', 'paused'].includes(box.state) && (
            <>
              <button className="button" onClick={() => setAction('Stop')}>
                <Square size={12} />
                Stop
              </button>
              <button className="button" onClick={() => setAction('Create checkpoint')}>
                <Plus size={15} />
                Create checkpoint
              </button>
            </>
          )}
          <button className="button" onClick={() => setAction('Delete')}>
            <Trash2 size={14} />
            Delete
          </button>
        </div>
      </header>
      {box.last_error && (
        <div className="notice danger">
          <CircleAlert size={18} />
          <span>{box.last_error}</span>
        </div>
      )}
      {box.state === 'paused' && (
        <div className="notice warning">
          <Pause size={17} />
          <span>
            vCPUs are frozen. RAM and disk are retained.{' '}
            <strong>Pausing does not free memory.</strong>
          </span>
        </div>
      )}
      {box.state === 'stopped' && (
        <div className="notice">
          <Square size={15} />
          <span>
            Disk retained. Previous RAM contents were discarded. Start boots this same disk.
          </span>
        </div>
      )}
      {box.restored_from && (
        <div className="notice restored-notice">
          <Check size={16} />
          <span>
            Restored from <strong>{box.restored_from}</strong>. Reconnect terminal and SSH sessions.
          </span>
        </div>
      )}
      <nav className="tabs" aria-label="Box sections">
        {tabs.map((tab) => (
          <button
            key={tab}
            aria-current={activeTab === tab.toLowerCase() ? 'page' : undefined}
            className={activeTab === tab.toLowerCase() ? 'active' : ''}
            onClick={() =>
              setParams((previous) => {
                const next = new URLSearchParams(previous)
                next.set('tab', tab.toLowerCase())
                return next
              })
            }
          >
            {tab}
          </button>
        ))}
      </nav>
      {activeTab === 'terminal' && (
        <>
          <div className="workspace-grid">
            <section className="terminal-panel">
              <div className="terminal-heading">
                <span>
                  <Terminal size={16} />
                  shell
                </span>
              </div>
              <CapabilitySurface capability="terminal" className="terminal-body">
                <div className="terminal-watermark">
                  <Terminal size={30} strokeWidth={1} />
                </div>
                <span className="terminal-comment"># {box.name} / workspace terminal</span>
                <p>A shell, right where you work.</p>
                <span className="terminal-comment">
                  {preview
                    ? '# Preview mode does not open a shell or execute commands.'
                    : '# Authenticated terminal transport is not configured.'}
                </span>
                <span className="terminal-prompt">
                  <span>$</span>
                  <span className="cursor-block" aria-hidden="true" />
                </span>
                <div className="terminal-connect">
                  <button className="button" disabled>
                    Connect terminal <ArrowUpRight size={14} />
                  </button>
                  <span>Requires the terminal service</span>
                </div>
              </CapabilitySurface>
            </section>
            <div className="access-column">
              <Access />
              <ResourceMetrics
                metrics={box.metrics}
                boxState={box.state}
                diskTotalBytes={box.resources?.disk_bytes ?? null}
                hostObservedDisk
              />
            </div>
          </div>
          <LaunchTiming box={box} />
        </>
      )}
      {activeTab === 'exec' && <Exec box={box} preview={preview} authenticated={authenticated} />}
      {activeTab === 'previews' && (
        <section className="panel preview-panel">
          <div className="panel-heading">
            <h2>HTTP previews</h2>
            <Lock size={15} />
          </div>
          <CapabilitySurface capability="httpPreview" className="preview-content">
            <Lock size={30} strokeWidth={1} />
            <h2>Private until you say otherwise.</h2>
            <p>
              Preview routes require the authenticated gateway. No ports are exposed by this
              dashboard.
            </p>
            <CapabilitySurface capability="publicUrl" className="form-row">
              <span>Public access</span>
              <span className="tag">Enabled</span>
            </CapabilitySurface>
            <button className="button" disabled>
              Add preview <Plus size={14} />
            </button>
          </CapabilitySurface>
        </section>
      )}
      {activeTab === 'checkpoints' && (
        <>
          <div className="section-toolbar">
            <span className="micro">Coordinated memory + disk / Same-box restore</span>
            <button
              className="button"
              disabled={!['running', 'paused'].includes(box.state)}
              onClick={() => setAction('Create checkpoint')}
            >
              <Plus size={14} />
              Create checkpoint
            </button>
          </div>
          {checkpoints.length === 0 ? (
            <Empty title="No checkpoints yet">
              Capture this box’s memory and disk before making changes.
            </Empty>
          ) : (
            <div className="checkpoint-list">
              {checkpoints.map((checkpoint) => (
                <div className="checkpoint" key={checkpoint.id}>
                  <HardDrive size={21} strokeWidth={1.3} />
                  <div>
                    <strong>{checkpoint.name}</strong>
                    <span className="resource-id">{timestamp(checkpoint.created_at)}</span>
                  </div>
                  <span className="micro">{formatBytes(checkpoint.size_bytes)}</span>
                  <span className="tag">
                    {checkpoint.compatible ? 'Compatible' : 'Incompatible'}
                  </span>
                  <button
                    className="button"
                    disabled={!checkpoint.compatible}
                    onClick={() => setRestoring(checkpoint)}
                    aria-label={`Restore ${checkpoint.name}`}
                  >
                    Restore <ArrowUpRight size={14} />
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="notice">
            <Info size={17} />
            <span>
              Restoring replaces this box’s memory and disk. It does not undo external side effects
              and does not create a clone.
            </span>
          </div>
        </>
      )}
      {activeTab === 'metrics' && (
        <>
          <ResourceMetrics
            metrics={box.metrics}
            boxState={box.state}
            diskTotalBytes={box.resources?.disk_bytes ?? null}
            hostObservedDisk
          />
          <LaunchTiming box={box} />
          {capabilityAvailable('metrics') && (
            <p className="section-note">
              Only host-reported measurements are shown. Missing data is never interpreted as zero
              usage.
            </p>
          )}
        </>
      )}
      {activeTab === 'activity' && (
        <OperationList
          operations={data.operations.filter((operation) => operation.box_id === box.id)}
        />
      )}
      {action === 'Delete' ? (
        <DeleteDialog
          box={box}
          preview={preview}
          authenticated={authenticated}
          onClose={() => setAction(null)}
        />
      ) : (
        action && (
          <LifecycleDialog
            action={action}
            box={box}
            preview={preview}
            authenticated={!preview && data.host.state === 'online'}
            onClose={() => setAction(null)}
          />
        )
      )}
      {restoring && (
        <RestoreDialog
          box={box}
          checkpoint={restoring}
          preview={preview}
          authenticated={authenticated}
          onClose={() => setRestoring(null)}
        />
      )}
    </>
  )
}

function Access() {
  return (
    <section className="panel access-panel">
      <div className="panel-heading">
        <h2>Access</h2>
        <ArrowUpRight size={15} />
      </div>
      <div className="access-section">
        <div className="split-line">
          <span className="eyebrow">SSH</span>
          <Lock size={14} />
        </div>
        <CapabilitySurface capability="ssh">
          <button className="button full-width" disabled>
            Connect
          </button>
        </CapabilitySurface>
      </div>
      <div className="access-section">
        <div className="split-line">
          <span className="eyebrow">HTTP preview</span>
          <Lock size={14} />
        </div>
        <CapabilitySurface capability="httpPreview">
          <button className="button full-width" disabled>
            Add preview
          </button>
        </CapabilitySurface>
      </div>
      <CapabilitySurface capability="publicUrl" className="access-bottom">
        <span className="micro">Public access</span>
        <span className="tag">Enabled</span>
      </CapabilitySurface>
    </section>
  )
}

function Exec({
  box,
  preview,
  authenticated,
}: {
  box: Box
  preview: boolean
  authenticated: boolean
}) {
  const [argv, setArgv] = useState('["/bin/pwd"]')
  const [cwd, setCwd] = useState('')
  const [timeout, setTimeoutMs] = useState('10000')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [output, setOutput] = useState<ExecOutput | null>(null)
  let command: string[] = []
  let valid = false
  try {
    const parsed: unknown = JSON.parse(argv)
    valid =
      Array.isArray(parsed) &&
      parsed.length > 0 &&
      parsed.every((item) => typeof item === 'string') &&
      parsed[0].length > 0
    if (valid) command = parsed as string[]
  } catch {
    valid = false
  }
  const timeoutMs = Number(timeout)
  const timeoutValid = Number.isInteger(timeoutMs) && timeoutMs >= 1 && timeoutMs <= 3_600_000
  const cwdValid = cwd === '' || (cwd.startsWith('/') && !cwd.includes('\0'))
  const ready =
    authenticated && box.state === 'running' && valid && timeoutValid && cwdValid && !pending
  const execAction = {
    type: 'exec',
    id: box.id,
    request: { argv: command, timeout_ms: timeoutMs, cwd: cwd || undefined },
  } as const
  return (
    <section className="panel exec-panel">
      <div className="panel-heading">
        <h2>Execute a command</h2>
        <Terminal size={16} />
      </div>
      <form
        onSubmit={(event) => {
          event.preventDefault()
          if (!ready) return
          setPending(true)
          setError('')
          setOutput(null)
          void execCommand(box.id, command, timeoutMs, cwd || undefined)
            .then((result) => {
              setOutput(result)
              setPending(false)
            })
            .catch((reason: unknown) => {
              setError(reason instanceof Error ? reason.message : 'unknown')
              setPending(false)
            })
        }}
      >
        <p>Structured arguments, bounded output. Commands are never retried automatically.</p>
        <div className="field">
          <label htmlFor="exec-argv">Arguments (JSON array)</label>
          <textarea
            id="exec-argv"
            spellCheck={false}
            value={argv}
            onChange={(event) => setArgv(event.target.value)}
            aria-invalid={!valid}
            aria-describedby="argv-hint"
          />
          <span id="argv-hint" className={`field-hint ${!valid ? 'error-text' : ''}`}>
            {valid
              ? 'No implicit shell expansion.'
              : 'Enter a non-empty JSON array of strings with a command as the first argument.'}
          </span>
        </div>
        <div className="exec-options">
          <div className="field">
            <label htmlFor="exec-cwd">Working directory</label>
            <input
              id="exec-cwd"
              placeholder="Guest default"
              value={cwd}
              onChange={(event) => setCwd(event.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="exec-timeout">Timeout (ms)</label>
            <input
              id="exec-timeout"
              type="number"
              min={1}
              max={3600000}
              value={timeout}
              onChange={(event) => setTimeoutMs(event.target.value)}
            />
          </div>
        </div>
        {authenticated ? (
          box.state === 'running' ? null : (
            <p className="field-hint">Commands run only while the box is running.</p>
          )
        ) : (
          <Unavailable preview={preview} />
        )}
        {pending && (
          <p className="field-hint" role="status">
            {operationPendingLabel(execAction)} Waiting for the command result.
          </p>
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
        {output && (
          <>
            <p>
              Exit code {output.exitCode ?? 'none'}
              {output.timedOut ? '. The command timed out.' : ''}
              {output.truncated ? ' Output was truncated.' : ''}
            </p>
            <pre aria-label="stdout">{output.stdout}</pre>
            <pre aria-label="stderr">{output.stderr}</pre>
          </>
        )}
        <button className="button primary" disabled={!ready} aria-busy={pending}>
          {pending ? (
            operationPendingLabel(execAction)
          ) : (
            <>
              Execute command <ArrowUpRight size={15} />
            </>
          )}
        </button>
      </form>
    </section>
  )
}

function LaunchTiming({ box }: { box: Box }) {
  const phase = (label: string, value: number | null | undefined) =>
    typeof value === 'number' ? { label, value } : null
  const phases = [
    phase('storage', box.timing?.storage_ms),
    phase('vmm', box.timing?.vmm_ms),
    phase('restore', box.timing?.restore_ms),
    phase('agent', box.timing?.agent_ms),
    phase('init', box.timing?.init_ms),
    phase('first command', box.timing?.first_command_ms),
  ].filter((row): row is { label: string; value: number } => row !== null)
  return (
    <section className="panel timing-panel">
      <div className="panel-heading">
        <h2>
          <Clock3 size={15} />
          Launch timing
        </h2>
        {capabilityAvailable('launchTiming') && (
          <span className="micro">
            {box.timing?.total_ms == null
              ? 'Not reported'
              : `${formatMetric(box.timing.total_ms, ' ms')} end to end`}
          </span>
        )}
      </div>
      <CapabilitySurface capability="launchTiming">
        <div className="timing-grid">
          {phases.length === 0 ? (
            <p className="micro">Phase breakdown not reported for this launch.</p>
          ) : (
            phases.map(({ label, value }) => (
              <div key={label}>
                <span className="eyebrow">{label}</span>
                <strong>{formatMetric(value, ' ms')}</strong>
                <span className="timing-tick" />
              </div>
            ))
          )}
        </div>
        <div className="timing-footer">
          <span>Request → first successful guest command</span>
          <span>
            {box.timing
              ? `${box.timing.method} / Cache: ${box.timing.cache} / Copy: ${box.timing.disk_copy}`
              : 'Cache and copy method not reported'}
          </span>
        </div>
      </CapabilitySurface>
    </section>
  )
}
