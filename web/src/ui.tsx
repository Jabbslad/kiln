import { useEffect, useId, useRef, useState, type ReactNode, type ComponentProps } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { ArrowUpRight, Box as BoxIcon, Check, Copy, Info, X } from 'lucide-react'
import { capabilities, capabilityAvailable, type Capability } from './capabilities'
import {
  boxStates,
  formatBytes,
  formatDiskUsedOfTotal,
  formatMetric,
  type Dashboard,
} from './data'

type BoxState = (typeof boxStates)[number]
type MetricRow = { label: string; value: string; hint: string }

function metricRows(
  metrics: Dashboard['host']['metrics'],
  boxState?: BoxState,
  diskTotalBytes: number | null = metrics.disk_total_bytes,
  hostObservedDisk = false,
): MetricRow[] {
  const active = !boxState || boxState === 'running' || boxState === 'paused'
  const inactiveLabel = boxState === 'stopped' ? 'Stopped' : 'Not available yet'
  const display = (value: number | null, format: (next: number) => string, hint = 'Observed use') => {
    if (value !== null) {
      return { value: format(value), hint }
    }
    if (!active) {
      return { value: inactiveLabel, hint: inactiveLabel }
    }
    return { value: 'Not reported', hint: 'Not reported' }
  }
  const diskHint = hostObservedDisk ? 'Host-observed' : 'Observed use'
  const disk = (() => {
    if (metrics.disk_bytes === null) {
      if (!active) {
        return { value: inactiveLabel, hint: inactiveLabel }
      }
      return { value: 'Not reported', hint: 'Not reported' }
    }
    const total = diskTotalBytes ?? metrics.disk_total_bytes
    const value =
      total !== null
        ? formatDiskUsedOfTotal(metrics.disk_bytes, total)
        : formatBytes(metrics.disk_bytes)
    return { value, hint: diskHint }
  })()
  return [
    { label: 'CPU', ...display(metrics.cpu_percent, (value) => formatMetric(value, '%')) },
    { label: 'Memory', ...display(metrics.memory_bytes, formatBytes) },
    { label: 'Disk', ...disk },
  ]
}

export type PageProps = {
  data: Dashboard
  preview: boolean
}

export function AppLink({ to, ...props }: ComponentProps<typeof Link>) {
  const [params] = useSearchParams()
  const destination =
    typeof to === 'string' && params.get('preview') === '1'
      ? `${to}${to.includes('?') ? '&' : '?'}preview=1`
      : to
  return <Link to={destination} {...props} />
}

export function Status({ state }: { state: string }) {
  return (
    <span className={`status status-${state}`}>
      <span className="status-dot" aria-hidden="true" />
      {state}
    </span>
  )
}

export function PageHeading({
  eyebrow,
  title,
  description,
  children,
}: {
  eyebrow: string
  title: string
  description: string
  children?: ReactNode
}) {
  return (
    <header className="page-heading">
      <div>
        <div className="eyebrow">{eyebrow}</div>
        <h1>{title}</h1>
        <p>{description}</p>
      </div>
      <div className="heading-actions">{children}</div>
    </header>
  )
}

export function Empty({
  title,
  children,
  action,
}: {
  title: string
  children: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="empty-state">
      <BoxIcon size={52} strokeWidth={1} aria-hidden="true" />
      <h2>{title}</h2>
      <p>{children}</p>
      {action}
    </div>
  )
}

export function NotAvailableYet() {
  return <p className="capability-unavailable">Not available yet</p>
}

export function CapabilitySurface({
  capability,
  children,
  className,
}: {
  capability: Capability
  children: ReactNode
  className?: string
}) {
  const status = capabilities[capability]
  return (
    <div
      className={className}
      data-capability={capability}
      data-capability-state={status}
      data-testid={`capability-${capability}`}
    >
      {status === 'not-yet' ? <NotAvailableYet /> : children}
    </div>
  )
}

export function Unavailable({ preview }: { preview: boolean }) {
  return (
    <div className="notice muted-notice">
      <Info size={16} aria-hidden="true" />
      <span>
        {preview
          ? 'Read-only preview. Example data; no VM operations are sent.'
          : 'Read-only console. Mutations require the authenticated control plane.'}
      </span>
    </div>
  )
}

export function ResourceMetrics({
  metrics,
  title = 'Resource use',
  boxState,
  diskTotalBytes = null,
  hostObservedDisk = false,
}: {
  metrics: Dashboard['host']['metrics']
  title?: string
  boxState?: BoxState
  diskTotalBytes?: number | null
  hostObservedDisk?: boolean
}) {
  const missing = [metrics.cpu_percent, metrics.memory_bytes, metrics.disk_bytes].every(
    (value) => value === null,
  )
  const active = !boxState || boxState === 'running' || boxState === 'paused'
  return (
    <section className="panel metrics-panel" aria-label={title}>
      <div className="panel-heading">
        <h2>{title}</h2>
        {capabilityAvailable('metrics') && (
          <span className="micro">
            {!active
              ? boxState === 'stopped'
                ? 'Box is stopped'
                : 'Telemetry unavailable'
              : missing
                ? 'Awaiting host telemetry'
                : 'Reported by host'}
          </span>
        )}
      </div>
      <CapabilitySurface capability="metrics">
        <div className="metric-grid">
          {metricRows(metrics, boxState, diskTotalBytes, hostObservedDisk).map(({ label, value, hint }) => (
            <div className="metric" key={label}>
              <span className="eyebrow">{label}</span>
              <strong>{value}</strong>
              <span className="micro">{hint}</span>
            </div>
          ))}
        </div>
      </CapabilitySurface>
    </section>
  )
}

export function Modal({
  title,
  children,
  onClose,
  drawer = false,
}: {
  title: string
  children: ReactNode
  onClose: () => void
  drawer?: boolean
}) {
  const ref = useRef<HTMLDialogElement>(null)
  const id = useId()
  useEffect(() => {
    const dialog = ref.current!
    const previous = document.activeElement as HTMLElement | null
    dialog.showModal()
    const overflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      dialog.close()
      document.body.style.overflow = overflow
      previous?.focus()
    }
  }, [])
  return (
    <dialog
      ref={ref}
      aria-labelledby={id}
      className={drawer ? 'modal drawer' : 'modal'}
      onCancel={(event) => {
        event.preventDefault()
        onClose()
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Tab') return
        const controls = Array.from(
          event.currentTarget.querySelectorAll<HTMLElement>(
            'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]',
          ),
        ).filter((control) => control.getClientRects().length > 0)
        const first = controls[0]
        const last = controls[controls.length - 1]
        if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault()
          first?.focus()
        } else if (event.shiftKey && document.activeElement === first) {
          event.preventDefault()
          last?.focus()
        }
      }}
    >
      <div className="modal-surface">
        <header className="modal-heading">
          <div>
            <div className="eyebrow">kiln / control plane</div>
            <h2 id={id}>{title}</h2>
          </div>
          <button className="icon-button" aria-label="Close dialog" onClick={onClose}>
            <X size={20} />
          </button>
        </header>
        {children}
      </div>
    </dialog>
  )
}

export function CopyButton({ value, label }: { value: string; label: string }) {
  const [status, setStatus] = useState('')
  return (
    <span className="copy-control">
      <button
        className="icon-button"
        aria-label={label}
        onClick={async () => {
          try {
            await navigator.clipboard.writeText(value)
            setStatus('Copied')
          } catch {
            setStatus('Copy unavailable. Select the text to copy.')
          }
        }}
      >
        {status === 'Copied' ? <Check size={15} /> : <Copy size={15} />}
      </button>
      <span className="sr-only" role="status">
        {status}
      </span>
    </span>
  )
}

export function CreateButton({ onClick }: { onClick: () => void }) {
  return (
    <button className="button primary" onClick={onClick}>
      Create box <ArrowUpRight size={16} aria-hidden="true" />
    </button>
  )
}
