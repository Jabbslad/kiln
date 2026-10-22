import { ArrowUpRight, BookOpen, KeyRound, Lock, Server, ShieldCheck } from 'lucide-react'
import { AppLink, PageHeading, type PageProps } from './ui'

export function Settings({ data, preview }: PageProps) {
  return (
    <>
      <PageHeading
        eyebrow="Workspace / configuration"
        title="Settings"
        description="The boundaries of your workspace. Explicit, not assumed."
      />
      <div className="settings-grid">
        <section className="panel">
          <div className="panel-heading">
            <h2>
              <Server size={16} />
              Workspace
            </h2>
            <span className="tag">Single host</span>
          </div>
          <dl className="property-list settings-properties">
            <div>
              <dt>Name</dt>
              <dd>{data.workspace}</dd>
            </div>
            <div>
              <dt>Host</dt>
              <dd>{data.host.name}</dd>
            </div>
            <div>
              <dt>Console mode</dt>
              <dd>{preview ? 'Design preview' : 'Read-only'}</dd>
            </div>
            <div>
              <dt>API contract</dt>
              <dd>Version 1</dd>
            </div>
          </dl>
        </section>
        <section className="panel">
          <div className="panel-heading">
            <h2>
              <KeyRound size={16} />
              API tokens
            </h2>
            <Lock size={14} />
          </div>
          <div className="settings-body">
            <h3>Scoped to your workspace.</h3>
            <p>
              Token administration requires the authenticated service. This console does not store
              credentials in the browser.
            </p>
            <button className="button" disabled>
              Create token <ArrowUpRight size={14} />
            </button>
          </div>
        </section>
        <section className="panel">
          <div className="panel-heading">
            <h2>
              <ShieldCheck size={16} />
              Network policy
            </h2>
          </div>
          <div className="settings-body">
            <h3>No implicit public access.</h3>
            <p>
              Host-enforced isolation, internet egress, SSH and preview routing are separate service
              capabilities. The dashboard does not configure host networking.
            </p>
            <span className="tag">Service integration required</span>
          </div>
        </section>
        <section className="panel">
          <div className="panel-heading">
            <h2>
              <BookOpen size={16} />
              Runtime contracts
            </h2>
          </div>
          <div className="settings-body">
            <h3>Know what each operation preserves.</h3>
            <p>
              Pause, stop, checkpoint and clone have different effects on memory, disk and instance
              identity.
            </p>
            <AppLink className="text-action" to="/docs">
              Read console guide <ArrowUpRight size={14} />
            </AppLink>
          </div>
        </section>
      </div>
    </>
  )
}

export function Docs() {
  return (
    <>
      <PageHeading
        eyebrow="Reference / console guide"
        title="Built around real state."
        description="A small guide to the contracts behind the controls."
      />
      <div className="docs-layout">
        <section className="panel prose">
          <h2>Four operations. Four contracts.</h2>
          <dl>
            <dt>Pause / resume</dt>
            <dd>
              Freeze vCPUs and continue the same instance. RAM and disks remain allocated. Pause is
              not hibernation.
            </dd>
            <dt>Stop / start</dt>
            <dd>
              Stop the VMM process and discard RAM. Start boots the same writable disk; it does not
              recover the old execution state.
            </dd>
            <dt>Checkpoint / restore</dt>
            <dd>
              Capture coordinated memory, VMM state and disk. Restore replaces the same box’s state,
              not external systems. Existing sessions may need to reconnect. Explicit rollback
              acknowledgement is required.
            </dd>
            <dt>Prepared-template clone</dt>
            <dd>
              Create a new identity and private writable disk from a controlled pre-workload
              template. Arbitrary live application forks are not provided.
            </dd>
          </dl>
        </section>
        <section className="panel prose">
          <h2>Connecting the console</h2>
          <p>
            This frontend reads <code>GET /api/v1/dashboard</code> from the same origin. A missing
            or incompatible response shows a connection error, never example data.
          </p>
          <p>
            Kiln has an authenticated CLI and HTTP API, but this dashboard’s aggregate endpoint and
            browser authentication are not integrated yet. Use the CLI for live VM operations. Do
            not expose private Firecracker sockets or administrator tokens to the browser.
          </p>
          <p>
            Preview mode is explicitly enabled with <code>?preview=1</code>. Its fixtures are
            read-only; no buttons create, stop or change a VM.
          </p>
          <h3>Before enabling mutations</h3>
          <p>
            The service must enforce workspace ownership, scoped credentials, durable idempotency,
            resource reservation, operation progress and audit events. The terminal opens a shell in
            a running box through a same-origin WebSocket when the session can operate. HTTP preview
            routing still needs authenticated gateway integration.
          </p>
          <h3>Source of truth</h3>
          <p>
            See <code>docs/runtime.md</code> for runtime behavior and <code>web/README.md</code> for
            the frontend contract and development commands.
          </p>
        </section>
      </div>
    </>
  )
}
