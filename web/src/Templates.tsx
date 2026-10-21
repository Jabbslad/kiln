import { useState } from 'react'
import { ArrowUpRight, Box, Check, Cpu, Info, Layers, ShieldCheck } from 'lucide-react'
import { formatBytes, type Template } from './data'
import { CreateDrawer } from './dialogs'
import { CreateButton, Empty, PageHeading, type PageProps } from './ui'

export default function Templates({ data, preview }: PageProps) {
  const [selected, setSelected] = useState(data.templates[0]?.id)
  const [creating, setCreating] = useState(false)
  const template = data.templates.find((item) => item.id === selected)
  return (
    <>
      <PageHeading
        eyebrow="Workspace / images"
        title="Templates"
        description="Prepared environments. A fresh identity for every box."
      >
        <CreateButton onClick={() => setCreating(true)} />
      </PageHeading>
      {data.templates.length === 0 ? (
        <Empty title="No prepared templates">
          Publish a template through the controlled template-builder path to get started.
        </Empty>
      ) : (
        <div className="templates-layout">
          <div className="template-list" aria-label="Prepared templates">
            {data.templates.map((item) => (
              <button
                className={`template-card ${selected === item.id ? 'selected' : ''}`}
                key={item.id}
                onClick={() => setSelected(item.id)}
                aria-pressed={selected === item.id}
              >
                <div className="template-card-top">
                  <span className="template-symbol">
                    <Layers size={24} strokeWidth={1.2} />
                  </span>
                  <span className={`template-state ${item.compatible ? 'green' : 'error-text'}`}>
                    <span className="status-dot" />
                    {item.compatible ? 'Compatible' : 'Incompatible'}
                  </span>
                </div>
                <div className="template-title">
                  {item.name}
                  <span>{item.version}</span>
                </div>
                <p>{item.description}</p>
                <div className="template-allocation">
                  <span>
                    <Cpu size={13} /> {item.resources.vcpus} vCPU
                  </span>
                  <span>{formatBytes(item.resources.memory_bytes)} RAM</span>
                  <span>{formatBytes(item.resources.disk_bytes)} disk</span>
                </div>
                <div className="template-card-bottom">
                  <span className="micro">
                    {item.development_only ? 'Trusted test fixture' : 'Prepared template'}
                  </span>
                  <span>
                    {selected === item.id ? <Check size={16} /> : <ArrowUpRight size={16} />}
                  </span>
                </div>
              </button>
            ))}
          </div>
          {template && (
            <TemplateDetail
              template={template}
              host={data.host.name}
              onCreate={() => setCreating(true)}
            />
          )}
        </div>
      )}
      <div className="notice template-note">
        <Info size={18} />
        <div>
          <strong>Templates and checkpoints do different jobs.</strong>
          <p>
            A prepared template creates a new box. A checkpoint restores memory and disk on the same
            box. Arbitrary live forks are not supported.
          </p>
        </div>
      </div>
      {creating && (
        <CreateDrawer
          data={data}
          preview={preview}
          authenticated={!preview && data.host.state === 'online'}
          initialTemplate={template}
          onClose={() => setCreating(false)}
        />
      )}
    </>
  )
}

function TemplateDetail({
  template,
  host,
  onCreate,
}: {
  template: Template
  host: string
  onCreate: () => void
}) {
  return (
    <aside className="panel template-inspector">
      <div className="panel-heading">
        <h2>Template details</h2>
        <Box size={16} />
      </div>
      <div className="inspector-body">
        <span className="eyebrow">{template.version} / x86_64</span>
        <h2>{template.name}</h2>
        <p>{template.description}</p>
        <dl className="property-list">
          <div>
            <dt>Host</dt>
            <dd>{host}</dd>
          </div>
          <div>
            <dt>Compatibility</dt>
            <dd className={template.compatible ? 'green' : 'error-text'}>
              {template.compatible ? 'Compatible' : 'Incompatible'}
            </dd>
          </div>
          <div>
            <dt>Disk</dt>
            <dd>Private, writable copy</dd>
          </div>
        </dl>
        <div className="eyebrow guarantee-heading">
          <ShieldCheck size={15} /> Launch contract
        </div>
        <ul className="contract-list">
          <li>
            <Check size={14} /> Captured before user workloads
          </li>
          <li>
            <Check size={14} /> Fresh identity and entropy
          </li>
          <li>
            <Check size={14} /> Secrets injected after restore
          </li>
          <li>
            <Check size={14} /> Readiness confirmed by guest
          </li>
        </ul>
        <button
          className="button primary full-width"
          disabled={!template.compatible}
          onClick={onCreate}
        >
          Use this template <ArrowUpRight size={16} />
        </button>
      </div>
    </aside>
  )
}
