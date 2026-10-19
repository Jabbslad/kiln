import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { ArrowUpRight, Check, ChevronDown, ChevronRight, Circle, CircleAlert } from 'lucide-react'
import { timestamp, type Operation } from './data'
import { AppLink, Empty, PageHeading, Status, type PageProps } from './ui'

export function OperationList({ operations }: { operations: Operation[] }) {
  const [params] = useSearchParams()
  const [expanded, setExpanded] = useState<string | null>(params.get('operation'))
  if (operations.length === 0)
    return (
      <Empty title="No operations yet">
        Lifecycle operations and their observed results will appear here.
      </Empty>
    )
  return (
    <div className="operation-list">
      {operations.map((operation) => (
        <article
          className={`operation ${expanded === operation.id ? 'expanded' : ''}`}
          key={operation.id}
        >
          <button
            className="operation-summary"
            aria-expanded={expanded === operation.id}
            aria-controls={`detail-${operation.id}`}
            onClick={() => setExpanded(expanded === operation.id ? null : operation.id)}
          >
            <span className={`operation-symbol status-${operation.state}`}>
              {operation.state === 'failed' ? (
                <CircleAlert size={19} />
              ) : operation.state === 'completed' ? (
                <Check size={19} />
              ) : (
                <Circle size={19} />
              )}
            </span>
            <span className="operation-label">
              <strong>{operation.kind}</strong>
              <span className="resource-id">
                {operation.box_name} <span className="separator">/</span> {operation.id}
              </span>
            </span>
            <span className="operation-time">{timestamp(operation.created_at)}</span>
            <Status state={operation.state} />
            {expanded === operation.id ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
          </button>
          {expanded === operation.id && (
            <div id={`detail-${operation.id}`} className="operation-detail">
              <p>{operation.detail}</p>
              <ol className="operation-steps">
                {operation.steps.map((step) => (
                  <li key={step.label} className={`step-${step.state}`}>
                    <span className="step-icon">
                      {step.state === 'completed' ? (
                        <Check size={13} />
                      ) : step.state === 'failed' ? (
                        <CircleAlert size={13} />
                      ) : (
                        <Circle size={11} />
                      )}
                    </span>
                    <span>{step.label}</span>
                    <span className="micro">{step.state}</span>
                  </li>
                ))}
              </ol>
              <AppLink
                className="text-action"
                to={`/boxes/${encodeURIComponent(operation.box_id)}?tab=activity`}
              >
                Inspect box <ArrowUpRight size={14} />
              </AppLink>
            </div>
          )}
        </article>
      ))}
    </div>
  )
}

export default function Operations({ data }: PageProps) {
  const [filter, setFilter] = useState('all')
  return (
    <>
      <PageHeading
        eyebrow="Workspace / activity"
        title="Operations"
        description="Every request has a result. Follow the work from intent to observed state."
      />
      <div className="section-toolbar">
        <div className="filters">
          {['all', 'running', 'completed', 'failed'].map((state) => (
            <button
              className={`filter ${filter === state ? 'selected' : ''}`}
              key={state}
              aria-pressed={filter === state}
              onClick={() => setFilter(state)}
            >
              {state}
            </button>
          ))}
        </div>
        <span className="micro">{data.operations.length} recorded operations</span>
      </div>
      <OperationList
        operations={data.operations.filter(
          (operation) => filter === 'all' || operation.state === filter,
        )}
      />
      <p className="section-note">
        A failed operation does not imply a stopped box. Inspect its observed state before taking
        action.
      </p>
    </>
  )
}
