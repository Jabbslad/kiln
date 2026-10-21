import { useEffect, useId, useRef, useState, type ReactNode, type ComponentProps } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { ArrowUpRight, Box as BoxIcon, Check, Copy, Info, X } from 'lucide-react'
import { formatBytes, formatMetric, type Dashboard } from './data'

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
}: {
  metrics: Dashboard['host']['metrics']
  title?: string
}) {
  const missing = Object.values(metrics).every((value) => value === null)
  return (
    <section className="panel metrics-panel" aria-label={title}>
      <div className="panel-heading">
        <h2>{title}</h2>
        <span className="micro">{missing ? 'Awaiting host telemetry' : 'Reported by host'}</span>
      </div>
      <div className="metric-grid">
        {[
          ['CPU', formatMetric(metrics.cpu_percent, '%')],
          ['Memory', formatBytes(metrics.memory_bytes)],
          ['Disk', formatBytes(metrics.disk_bytes)],
        ].map(([label, value]) => (
          <div className="metric" key={label}>
            <span className="eyebrow">{label}</span>
            <strong>{value}</strong>
            <span className="micro">{value === '—' ? 'Not reported' : 'Observed use'}</span>
          </div>
        ))}
      </div>
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
