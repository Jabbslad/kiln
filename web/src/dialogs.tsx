import { useState } from 'react'
import { AlertTriangle, ArrowUpRight, Check, Lock, ShieldCheck } from 'lucide-react'
import {
  applyLifecycle,
  createBox,
  createCheckpoint,
  deleteBox,
  formatBytes,
  operationPendingLabel,
  requestId,
  restoreCheckpoint,
  type Box,
  type Checkpoint,
  type Dashboard,
  type OperationAction,
  type Template,
} from './data'
import { apiErrorMessage } from './kilnFetch'
import { Modal, Unavailable } from './ui'

export function CreateDrawer({
  data,
  preview,
  authenticated,
  initialTemplate,
  onClose,
}: {
  data: Dashboard
  preview: boolean
  authenticated: boolean
  initialTemplate?: Template
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
  const createAction = { type: 'create', template: template?.name ?? '', name } as const
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
            .then(() => onClose())
            .catch((reason: unknown) => {
              setError(apiErrorMessage(reason, 'Create did not finish.'))
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
          {pending && (
            <p className="field-hint" role="status">
              {operationPendingLabel(createAction)} Waiting for the control plane to finish booting
              the box.
            </p>
          )}
          {error && (
            <p className="field-hint error-text" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="modal-footer">
          <button type="button" className="button" onClick={onClose} disabled={pending}>
            Cancel
          </button>
          <button className="button primary" disabled={!ready} aria-busy={pending}>
            {pending ? (
              operationPendingLabel(createAction)
            ) : (
              <>
                Create box <ArrowUpRight size={16} />
              </>
            )}
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
  onClose,
}: {
  box: Box
  checkpoint: Checkpoint
  preview: boolean
  authenticated?: boolean
  onClose: () => void
}) {
  const [acknowledged, setAcknowledged] = useState(false)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const [operationId] = useState(() => requestId())
  const sameBox = checkpoint.box_id === box.id
  const ready = authenticated && acknowledged && sameBox && !pending
  const restoreAction = {
    type: 'restore',
    id: box.id,
    checkpoint: checkpoint.id,
    acknowledge_external_state_replay: true,
  } as const
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
        {pending && (
          <p className="field-hint" role="status">
            {operationPendingLabel(restoreAction)} Waiting for the control plane to observe the
            restore.
          </p>
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose} disabled={pending}>
          Cancel
        </button>
        <button
          className="button primary"
          disabled={!ready}
          aria-busy={pending}
          aria-describedby="restore-gate"
          onClick={() => {
            if (!ready) return
            setPending(true)
            setError('')
            void restoreCheckpoint(box.id, checkpoint.id, fetch, document.cookie, operationId)
              .then(() => onClose())
              .catch((reason: unknown) => {
                setError(apiErrorMessage(reason, 'The operation did not finish.'))
                setPending(false)
              })
          }}
        >
          {pending ? operationPendingLabel(restoreAction) : 'Restore checkpoint'}
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
  onClose,
}: {
  action: string
  box: Box
  preview: boolean
  authenticated?: boolean
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
  const lifecycleAction = checkpoint
    ? ({ type: 'checkpoint', id: box.id } as const)
    : action === 'Stop'
      ? ({ type: 'stop', id: box.id, force } as const)
      : ({ type: action.toLowerCase(), id: box.id } as OperationAction)
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
        {pending && (
          <p className="field-hint" role="status">
            {operationPendingLabel(lifecycleAction)} Waiting for the control plane to observe the
            change.
          </p>
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose} disabled={pending}>
          Cancel
        </button>
        <button
          className="button primary"
          disabled={!ready}
          aria-busy={pending}
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
              .then(() => onClose())
              .catch((reason: unknown) => {
                setError(apiErrorMessage(reason, 'The operation did not finish.'))
                setPending(false)
              })
          }}
        >
          {pending ? operationPendingLabel(lifecycleAction) : action}
        </button>
      </footer>
    </Modal>
  )
}

export function DeleteDialog({
  box,
  preview,
  authenticated = false,
  onClose,
}: {
  box: Box
  preview: boolean
  authenticated?: boolean
  onClose: () => void
}) {
  const [typed, setTyped] = useState('')
  const [pending, setPending] = useState(false)
  const [error, setError] = useState('')
  const ready = authenticated && typed === box.name && !pending
  const deleteAction = { type: 'delete', id: box.id } as const
  return (
    <Modal title="Delete box?" onClose={onClose}>
      <div className="modal-body">
        <div className="restore-target">{box.name}</div>
        <p className="body-copy">
          Permanently delete this box and its writable disk. Type the box name to confirm.
        </p>
        <div className="field">
          <label htmlFor="delete-name">Type the box name</label>
          <input
            id="delete-name"
            value={typed}
            autoComplete="off"
            disabled={pending}
            onChange={(event) => setTyped(event.target.value)}
          />
        </div>
        {authenticated ? (
          <p className="field-hint">
            The box leaves the inventory after the control plane observes it gone.
          </p>
        ) : (
          <Unavailable preview={preview} />
        )}
        {pending && (
          <p className="field-hint" role="status">
            {operationPendingLabel(deleteAction)} Waiting for the control plane to observe removal.
          </p>
        )}
        {error && (
          <p className="field-hint error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose} disabled={pending}>
          Cancel
        </button>
        <button
          className="button primary"
          disabled={!ready}
          aria-busy={pending}
          onClick={() => {
            if (!ready) return
            setPending(true)
            setError('')
            void deleteBox(box.id)
              .then(() => onClose())
              .catch((reason: unknown) => {
                setError(apiErrorMessage(reason, 'The operation did not finish.'))
                setPending(false)
              })
          }}
        >
          {pending ? operationPendingLabel(deleteAction) : 'Delete box'}
        </button>
      </footer>
    </Modal>
  )
}
