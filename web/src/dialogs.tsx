import { useState } from 'react'
import { AlertTriangle, ArrowUpRight, Check, Lock, ShieldCheck } from 'lucide-react'
import { formatBytes, type Box, type Checkpoint, type Dashboard, type Template } from './data'
import { Modal, Unavailable } from './ui'

export function CreateDrawer({
  data,
  preview,
  initialTemplate,
  onClose,
}: {
  data: Dashboard
  preview: boolean
  initialTemplate?: Template
  onClose: () => void
}) {
  const [templateId, setTemplateId] = useState(
    initialTemplate?.id ?? data.templates.find((item) => item.compatible)?.id ?? '',
  )
  const [name, setName] = useState('')
  const [egress, setEgress] = useState(false)
  const template = data.templates.find((item) => item.id === templateId)
  const invalidName = name.length > 0 && !/^[a-z][a-z0-9-]{0,62}$/.test(name)
  const duplicateName = data.boxes.some((box) => box.name === name)
  return (
    <Modal title="Create box" drawer onClose={onClose}>
      <form onSubmit={(event) => event.preventDefault()}>
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
          <Unavailable preview={preview} />
        </div>
        <footer className="modal-footer">
          <button type="button" className="button" onClick={onClose}>
            Cancel
          </button>
          <button className="button primary" disabled>
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
  onClose,
}: {
  box: Box
  checkpoint: Checkpoint
  preview: boolean
  onClose: () => void
}) {
  const [acknowledged, setAcknowledged] = useState(false)
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
          I understand the rollback risks.
        </label>
        <Unavailable preview={preview} />
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose}>
          Cancel
        </button>
        <button className="button primary" disabled aria-describedby="restore-gate">
          Restore checkpoint
        </button>
      </footer>
      <p id="restore-gate" className="dialog-footnote">
        {acknowledged
          ? 'Acknowledged. Restore remains unavailable without the authenticated service.'
          : 'Acknowledge the risks before restoring through the authenticated service.'}
      </p>
    </Modal>
  )
}

export function LifecycleDialog({
  action,
  box,
  preview,
  onClose,
}: {
  action: string
  box: Box
  preview: boolean
  onClose: () => void
}) {
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
        <Unavailable preview={preview} />
      </div>
      <footer className="modal-footer">
        <button className="button" onClick={onClose}>
          Cancel
        </button>
        <button className="button primary" disabled>
          {action}
        </button>
      </footer>
    </Modal>
  )
}
