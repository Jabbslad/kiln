import { useEffect, useState } from 'react'
import { Link, Route, Routes, useLocation, useSearchParams } from 'react-router-dom'
import {
  ArrowUpRight,
  Box,
  BookOpen,
  ChevronDown,
  CircleAlert,
  Command,
  ExternalLink,
  FlaskConical,
  Layers,
  Radio,
  RefreshCw,
  Server,
  Settings as SettingsIcon,
  SlidersHorizontal,
  UserRound,
} from 'lucide-react'
import { fetchDashboard, readCsrf, setDashboardObserver, timestamp, type Dashboard } from './data'
import { previewDashboard } from './fixtures'
import Inventory from './Inventory'
import BoxDetail from './BoxDetail'
import Templates from './Templates'
import Operations from './Operations'
import { Docs, Settings } from './Settings'
import { AppLink, Empty } from './ui'
import Landing from './Landing'
import { kilnVersionLabel } from './kiln-version'
import { ReleaseNotesDialog } from './ReleaseNotesDialog'

type Connection =
  { state: 'loading' } | { state: 'error'; message: string } | { state: 'ready'; data: Dashboard }
async function signOut() {
  const response = await fetch('/logout', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: `csrf=${readCsrf(document.cookie)}`,
  })
  if (response.ok) window.location.assign('/login')
}

const navigation = [
  { path: '/boxes', label: 'Boxes', icon: Box },
  { path: '/templates', label: 'Templates', icon: Layers },
  { path: '/operations', label: 'Operations', icon: SlidersHorizontal },
  { path: '/settings', label: 'Settings', icon: SettingsIcon },
]

export default function App() {
  const location = useLocation()
  return location.pathname === '/' ? <Landing /> : <Dashboard />
}

function Dashboard() {
  const [params] = useSearchParams()
  const location = useLocation()
  const preview = params.get('preview') === '1'
  const [connection, setConnection] = useState<Connection>({ state: 'loading' })
  const [attempt, setAttempt] = useState(0)
  const [releaseNotesOpen, setReleaseNotesOpen] = useState(false)
  useEffect(() => {
    if (preview) return
    setConnection({ state: 'loading' })
    const controller = new AbortController()
    let active = true
    const timeout = window.setTimeout(() => {
      if (active) {
        active = false
        controller.abort()
        setConnection({
          state: 'error',
          message:
            'The dashboard API did not respond within 10 seconds. No box state has been inferred.',
        })
      }
    }, 10000)
    fetchDashboard(controller.signal)
      .then((data) => {
        if (active) setConnection({ state: 'ready', data })
      })
      .catch((error) => {
        if (active)
          setConnection({
            state: 'error',
            message: error instanceof Error ? error.message : 'Unable to reach the dashboard API.',
          })
      })
      .finally(() => window.clearTimeout(timeout))
    return () => {
      active = false
      controller.abort()
      window.clearTimeout(timeout)
    }
  }, [preview, attempt])
  const data = preview ? previewDashboard : connection.state === 'ready' ? connection.data : null
  useEffect(() => {
    if (preview) return
    setDashboardObserver((next) => setConnection({ state: 'ready', data: next }))
    return () => setDashboardObserver(undefined)
  }, [preview])
  const routeLabel =
    navigation.find((item) => location.pathname.startsWith(item.path))?.label ?? 'Guide'
  useEffect(() => {
    document.title = `${routeLabel} — kiln`
  }, [routeLabel])
  const failedCount =
    data?.operations.filter((operation) => operation.state === 'failed').length ?? 0
  return (
    <div className="app">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="topbar">
        <div className="brand">
          <Link to="/" aria-label="kiln home">
            kiln<span className="brand-period">.</span>
          </Link>
          <span className="brand-divider" />
          <span className="topbar-label">Single-host console</span>
        </div>
        <div className="topbar-right">
          <span className={`connection-label ${preview ? 'preview-label' : ''}`}>
            {preview ? <FlaskConical size={13} /> : <Radio size={13} />}
            {preview
              ? 'Design preview'
              : data
                ? 'Read-only connection'
                : connection.state === 'loading'
                  ? 'Connecting'
                  : 'Not connected'}
          </span>
          <button
            type="button"
            className="topbar-version"
            aria-label={`Release notes for ${kilnVersionLabel(data?.version)}`}
            onClick={() => setReleaseNotesOpen(true)}
          >
            {kilnVersionLabel(data?.version)}
          </button>
          {releaseNotesOpen ? (
            <ReleaseNotesDialog
              displayVersion={data?.version}
              onClose={() => setReleaseNotesOpen(false)}
            />
          ) : null}
          {data && !preview && (
            <button className="button" type="button" onClick={() => void signOut()}>
              Sign out
            </button>
          )}
        </div>
      </header>
      <aside id="sidebar" className="sidebar">
        <div className="workspace-switch">
          <span className="workspace-avatar">{(data?.workspace ?? 'Workspace')[0]}</span>
          <div>
            <strong>{data?.workspace ?? 'Workspace'}</strong>
            <span>{data?.host.name ?? 'No host connected'}</span>
          </div>
          <ChevronDown size={14} aria-hidden="true" />
        </div>
        <div className="sidebar-label">Workspace</div>
        <nav aria-label="Main navigation">
          {navigation.map(({ path, label, icon: Icon }) => (
            <AppLink
              key={path}
              to={path}
              className={`nav-item ${location.pathname.startsWith(path) ? 'active' : ''}`}
              aria-current={location.pathname.startsWith(path) ? 'page' : undefined}
            >
              <Icon size={17} strokeWidth={1.5} />
              <span>{label}</span>
              {label === 'Boxes' && data && (
                <span className="nav-count">{data.boxes.length.toString().padStart(2, '0')}</span>
              )}
              {label === 'Operations' && failedCount > 0 && (
                <span className="attention-dot" aria-label={`${failedCount} failed operations`} />
              )}
            </AppLink>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="host-card">
            <Server size={16} />
            <span>{data?.host.name ?? 'Host unavailable'}</span>
            <span className={`status-dot ${data?.host.state === 'online' ? 'green' : ''}`} />
            <small>
              {preview
                ? 'EXAMPLE HOST'
                : data?.host.state === 'online'
                  ? 'REPORTED ONLINE'
                  : 'NO HOST CONNECTION'}
            </small>
          </div>
          <AppLink
            className={`nav-item ${location.pathname === '/docs' ? 'active' : ''}`}
            to="/docs"
          >
            <BookOpen size={16} />
            Console guide
            <ArrowUpRight size={12} />
          </AppLink>
          <div className="admin-row">
            <span className="admin-avatar">
              <UserRound size={14} />
            </span>
            <div>
              {preview ? 'Administrator' : 'Authentication pending'}
              <small>{preview ? 'Example account' : 'No identity configured'}</small>
            </div>
          </div>
        </div>
      </aside>
      <div className="content-shell">
        <div className="contextbar">
          <div>
            <span className="muted">Workspace</span>
            <span>/</span>
            <span>{data?.workspace ?? 'Not connected'}</span>
            <span>/</span>
            <span>{routeLabel}</span>
          </div>
          <div>
            <span className="contextbar-note">
              {preview
                ? 'EXAMPLE DATA · NO VM OPERATIONS'
                : data
                  ? timestamp(data.observed_at)
                  : 'AWAITING CONTROL PLANE'}
            </span>
            {preview ? (
              <Link className="context-link" to="/boxes">
                Exit preview <ExternalLink size={11} />
              </Link>
            ) : (
              <button
                className="icon-button"
                aria-label="Refresh dashboard"
                disabled={connection.state === 'loading'}
                onClick={() => setAttempt((value) => value + 1)}
              >
                <RefreshCw size={13} />
              </button>
            )}
          </div>
        </div>
        <main id="main" tabIndex={-1}>
          <Routes>
            <Route path="/docs" element={<Docs />} />
            <Route
              path="*"
              element={
                data ? (
                  <Routes>
                    <Route path="/boxes" element={<Inventory data={data} preview={preview} />} />
                    <Route
                      path="/boxes/:id"
                      element={<BoxDetail key={location.pathname} data={data} preview={preview} />}
                    />
                    <Route
                      path="/templates"
                      element={<Templates data={data} preview={preview} />}
                    />
                    <Route
                      path="/operations"
                      element={<Operations data={data} preview={preview} />}
                    />
                    <Route path="/settings" element={<Settings data={data} preview={preview} />} />
                    <Route
                      path="*"
                      element={
                        <Empty
                          title="Page not found"
                          action={
                            <AppLink className="button" to="/boxes">
                              Back to boxes
                            </AppLink>
                          }
                        >
                          This page isn’t part of the console.
                        </Empty>
                      }
                    />
                  </Routes>
                ) : connection.state === 'loading' ? (
                  <Loading />
                ) : (
                  <Disconnected
                    message={connection.state === 'error' ? connection.message : ''}
                    retry={() => setAttempt((value) => value + 1)}
                  />
                )
              }
            />
          </Routes>
        </main>
        <footer className="app-footer">
          <span>
            kiln <span className="separator">/</span> Your compute, under your control.
          </span>
          <span>
            <Command size={11} />
            {preview ? 'READ-ONLY DESIGN PREVIEW' : 'SINGLE-HOST CONSOLE'}
          </span>
        </footer>
      </div>
    </div>
  )
}

function Loading() {
  return (
    <div className="loading-state" role="status">
      <div className="eyebrow">Workspace / compute</div>
      <h1>Fetching box state…</h1>
      <p>Waiting for the host. No state inferred.</p>
      <div className="skeleton-table" aria-hidden="true">
        {[1, 2, 3, 4].map((row) => (
          <div key={row}>
            <span />
            <span />
            <span />
          </div>
        ))}
      </div>
    </div>
  )
}

function Disconnected({ message, retry }: { message: string; retry: () => void }) {
  return (
    <div className="disconnected">
      <div className="connection-diagram" aria-hidden="true">
        <Box size={38} strokeWidth={1} />
        <span />
        <CircleAlert size={17} />
        <span />
        <Server size={38} strokeWidth={1} />
      </div>
      <span className="eyebrow">Console ready / service unavailable</span>
      <h1>Connect your control plane</h1>
      <p>
        The dashboard is ready. Your boxes will appear once the authenticated service is connected.
      </p>
      <div className="connection-error">
        <CircleAlert size={16} />
        <span>{message}</span>
      </div>
      <div className="disconnected-actions">
        <a className="button primary" href="/login">
          Sign in
        </a>
        <button className="button" onClick={retry}>
          <RefreshCw size={14} />
          Retry connection
        </button>
        <Link className="button" to="/boxes?preview=1">
          Explore design preview <ArrowUpRight size={15} />
        </Link>
      </div>
      <p className="connection-footnote">
        Preview uses labelled, read-only examples. Nothing is created on your host.
      </p>
      <AppLink className="text-action" to="/docs">
        Connection guide <ArrowUpRight size={14} />
      </AppLink>
    </div>
  )
}
