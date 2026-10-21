import { useState } from 'react'
import { AlertTriangle, ArrowUpRight, Check, Lock, ShieldCheck } from 'lucide-react'
import {
  applyLifecycle,
  createBox,
  createCheckpoint,
  formatBytes,
  requestId,
  restoreCheckpoint,
  type Box,
  type Checkpoint,
  type Dashboard,
  type Template,
} from './data'
import { Modal, Unavailable } from './ui'

export function CreateDrawer({
  data,
  preview,
  authenticated,
  initialTemplate,
  onObserved,
  onClose,
}: {
  data: Dashboard
  preview: boolean
  authenticated: boolean
  initialTemplate?: Template
  onObserved?: (data: Dashboard) => void
  onClose: () => void
}) {
  const [templateId, setTemplateId] = useState(
    initialTemplate?.id ?? data.templates.find((item) => item.compatible)?.id ?? '',
  )
  const [name, setName] = useState('')
  const [egress, setEgress] = useState(false)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const template = data.templates.find((item) => item.id === templateId)
  const invalidName = name.length > 0 && !/^[a-z][a-z0-9-]{0,62}$/.test(name)
  const duplicateName = data.boxes.some((box) => box.name === name)
  const ready =
    authenticated &&
    !pending &&
    Boolean(template?.compatible) &&
    name.length > 0 &&
    !invalidName &&
    !duplicateName
  return (
    <Modal title="Create box" drawer onClose={onClose}>
      <form
        onSubmit={(event) => {
          event.preventDefault()
          if (!ready || !template) return
          setPending(true)
          setError('')
          void createBox(template.name, name)
            .then((observed) => {
              onObserved?.(observed)
              onClose()
            })
            .catch((reason: unknown) => {
              setError(reason instanceof Error ? reason.message : 'Create did not finish.')
              setPending(false)
            })
        }}
      >
        <div className="modal-body">
          <p className="form-intro">
            A fresh environment. An independent disk.
            <br />
            Ready for whatever you’re building.
          </p>
          <div className="field">
            <label htmlFor="template">Prepared template</label>
            <select
              id="template"
              value={templateId}
              onChange={(event) => setTemplateId(event.target.value)}
            >
              <option value="" disabled>
                Select a template
              </option>
              {data.templates.map((item) => (
                <option key={item.id} value={item.id} disabled={!item.compatible}>
                  {item.name} / {item.version}
                  {!item.compatible ? ' · incompatible' : ''}
                </option>
              ))}
            </select>
            <span className="field-hint">Checkpoints cannot be used as templates.</span>
          </div>
          <div className="field">
            <label htmlFor="box-name">Box name</label>
            <input
              id="box-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="e.g. api-staging"
              autoComplete="off"
              maxLength={63}
              aria-invalid={invalidName || duplicateName}
              aria-describedby="name-hint"
            />
            <span
              id="name-hint"
              className={`field-hint ${invalidName || duplicateName ? 'error-text' : ''}`}
            >
              {invalidName
                ? 'Use lowercase letters, numbers and hyphens. Start with a letter.'
                : duplicateName
                  ? 'A box with this name already exists.'
                  : 'A memorable name for your environment.'}
            </span>
          </div>
          <section className="form-section">
            <h3>Template allocation</h3>
            <div className="allocation-grid">
              <div>
                <span>vCPU</span>
                <strong>{template?.resources.vcpus ?? '—'}</strong>
              </div>
              <div>
                <span>Memory</span>
                <strong>{formatBytes(template?.resources.memory_bytes ?? null)}</strong>
              </div>
              <div>
                <span>Disk</span>
                <strong>{formatBytes(template?.resources.disk_bytes ?? null)}</strong>
              </div>
            </div>
            <p className="field-hint">Resources are fixed by the prepared template.</p>
          </section>
          <section className="form-section">
            <h3>Placement</h3>
            <div className="form-row">
              <span className="mono">{data.host.name}</span>
              <span className="micro">
                <ShieldCheck size={14} />{' '}
                {template?.compatible ? 'Compatible template' : 'Select a compatible template'}
              </span>
            </div>
          </section>
          <section className="form-section">
            <h3>Network policy</h3>
            <label className="switch-row">
              <span>Internet egress</span>
              <input
                className="switch-input"
                type="checkbox"
                checked={egress}
                onChange={(event) => setEgress(event.target.checked)}
              />
              <span className="switch" aria-hidden="true" />
              <span className="micro">{egress ? 'On' : 'Off'}</span>
            </label>
            <p className="field-hint">
              Host management endpoints and other boxes remain blocked. This form does not apply
              policy until the service is connected.
            </p>
            <div className="form-row">
              <span>HTTP previews</span>
              <span className="micro">
                <Lock size={13} /> Workspace only
              </span>
            </div>
          </section>
          <section className="form-section">
            <h3>Secrets</h3>
            <div className="secrets-empty">
              <Lock size={18} />
              <p>No secrets attached</p>
              <span className="field-hint">
                Injected after restore, never baked into a template.
                <br />
                Secret administration requires the service.
              </span>
            </div>
          </section>
          {template?.development_only && (
            <div className="notice warning">
              <AlertTriangle size={16} />
              <span>Trusted test fixture. Not an environment for untrusted workloads.</span>
            </div>
          )}
          {authenticated ? (
            <p className="field-hint">The box appears after the control plane observes it.</p>
          ) : (
            <Unavailable preview={preview} />
          )}
          {error && (
            <p className="field-hint error-text" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="modal-footer">
          <button type="button" className="button" onClick={onClose}>
            Cancel
          </button>
          <button className="button primary" disabled={!ready}>
            Create box <ArrowUpRight size={16} />
          </button>
        </footer>
      </form>
    </Modal>
  )
}

export function RestoreDialog({
  box,
  checkpoint,
  preview,
  authenticated = false,
  onObserved,
  onClose,
}: {
  box: Box
  checkpoint: Checkpoint
  preview: boolean
  authenticated?: boolean
  onObserved?: (data: Dashboard) => void
  onClose: () => void
}) {
  const [acknowledged, setAcknowledged] = useState(false)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [operationId] = useState(() => requestId())
  const sameBox = checkpoint.box_id === box.id
  const ready = authenticated && acknowledged && sameBox && !pending
  return (
    <Modal title="Restore checkpoint?" onClose={onClose}>
      <div className="modal-body">
        <div className="restore-target">
          <span>{box.name}</span>
          <span className="muted">/</span>
          <span>{checkpoint.name}</span>
        </div>
        <div className="notice warning">
          <AlertTriangle size={19} />
          <div>
            <strong>This replaces current memory and disk.</strong>
            <p>
              External actions are not undone. Sessions may disconnect; tokens and application state
              may be replayed.
            </p>
          </div>
        </div>
        <ul className="contract-list">
          <li>
            <Check size={15} /> Restore this box, not a new clone.
          </li>
          <li>
            <Check size={15} /> Reconnect terminal and SSH sessions afterwards.
          </li>
        </ul>
        <label className="acknowledgement">
          <input
            type="checkbox"
            checked={acknowledged}
            onChange={(event) => setAcknowledged(event.target.checked)}
          />
          I understand external state may replay.
        </label>
        {authenticated ? (
          <p className="field-hint">
            This box updates after the control plane observes the restore.
          </p>
        ) : (
          <Unavailable preview={preview} />
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose}>
          Cancel
        </button>
        <button
          className="button primary"
          disabled={!ready}
          aria-describedby="restore-gate"
          onClick={() => {
            if (!ready) return
            setPending(true)
            setError('')
            void restoreCheckpoint(box.id, checkpoint.id, fetch, document.cookie, operationId)
              .then((observed) => {
                onObserved?.(observed)
                onClose()
              })
              .catch((reason: unknown) => {
                setError(reason instanceof Error ? reason.message : 'unknown')
                setPending(false)
              })
          }}
        >
          Restore checkpoint
        </button>
      </footer>
      <p id="restore-gate" className="dialog-footnote">
        {authenticated && acknowledged
          ? 'Same-box restore. External state may replay.'
          : 'Acknowledge that external state may replay before restoring.'}
      </p>
    </Modal>
  )
}

export function LifecycleDialog({
  action,
  box,
  preview,
  authenticated = false,
  onObserved,
  onClose,
}: {
  action: string
  box: Box
  preview: boolean
  authenticated?: boolean
  onObserved?: (data: Dashboard) => void
  onClose: () => void
}) {
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [operationId] = useState(() => requestId())
  const [force, setForce] = useState(false)
  const checkpoint = action === 'Create checkpoint'
  const lifecycle =
    action === 'Pause' || action === 'Resume' || action === 'Stop' || action === 'Start'
  const ready = authenticated && (checkpoint || lifecycle) && !pending
  const explanations: Record<string, string> = {
    Pause:
      'Freeze vCPUs while keeping the VMM process, RAM and disk. Pausing does not release memory.',
    Resume: 'Continue this same instance from its paused execution state.',
    Stop: 'Stop the VMM process. The writable disk is preserved, but the current contents of RAM are lost.',
    Start:
      'Boot this box from its existing writable disk. The previous contents of RAM are not restored.',
    'Create checkpoint':
      'Capture coordinated memory and disk state. A running box is briefly paused during capture, then resumed. External side effects are not captured.',
  }
  return (
    <Modal title={`${action}${action === 'Create checkpoint' ? '' : ' box'}?`} onClose={onClose}>
      <div className="modal-body">
        <div className="restore-target">{box.name}</div>
        <p className="body-copy">{explanations[action]}</p>
        {action === 'Stop' && (
          <label className="acknowledgement">
            <input
              type="checkbox"
              checked={force}
              disabled={pending}
              onChange={(event) => setForce(event.target.checked)}
            />
            Force stop
          </label>
        )}
        {(checkpoint || lifecycle) && authenticated ? (
          <p className="field-hint">
            {checkpoint
              ? 'The checkpoint appears after the control plane observes it.'
              : 'The box state updates after the control plane observes it.'}
          </p>
        ) : (
          <Unavailable preview={preview} />
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose}>
          Cancel
        </button>
        <button
          className="button primary"
          disabled={!ready}
          onClick={() => {
            if (!ready) return
            setPending(true)
            setError('')
            const task = checkpoint
              ? createCheckpoint(box.id, fetch, document.cookie, operationId)
              : applyLifecycle(
                  action.toLowerCase() as 'pause' | 'resume' | 'stop' | 'start',
                  box.id,
                  action === 'Stop' && force,
                )
            void task
              .then((observed) => {
                onObserved?.(observed)
                onClose()
              })
              .catch((reason: unknown) => {
                setError(reason instanceof Error ? reason.message : 'unknown')
                setPending(false)
              })
          }}
        >
          {action}
        </button>
      </footer>
    </Modal>
  )
}
