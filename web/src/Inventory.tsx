import { useState } from 'react'
import { ArrowDown, ArrowUp, ArrowUpRight, CircleAlert, Cpu, Search, X } from 'lucide-react'
import { boxStates, filterBoxes, formatBytes, formatDiskUsedOfTotal, type BoxState } from './data'
import {
  AppLink,
  CreateButton,
  Empty,
  PageHeading,
  ResourceMetrics,
  Status,
  type PageProps,
} from './ui'
import { CreateDrawer } from './dialogs'

export default function Inventory({ data }: PageProps) {
  const [search, setSearch] = useState('')
  const [filter, setFilter] = useState<BoxState | 'all'>('all')
  const [descending, setDescending] = useState(false)
  const [creating, setCreating] = useState(false)
  const boxes = filterBoxes(data.boxes, search, filter).sort((a, b) =>
    descending ? b.name.localeCompare(a.name) : a.name.localeCompare(b.name),
  )
  const failed = data.operations.find((operation) => operation.state === 'failed')
  const reset = () => {
    setSearch('')
    setFilter('all')
  }
  return (
    <>
      <PageHeading
        eyebrow="Workspace / compute"
        title="Boxes"
        description="Your persistent Linux computers. Pick up where you left off."
      >
        <CreateButton onClick={() => setCreating(true)} />
      </PageHeading>
      <div className="inventory-overline">
        <span className="eyebrow">
          <span className="little-square" />
          {data.host.name}
        </span>
        <span className="micro">
          {data.boxes.length} environments <span className="separator">/</span> Single-host
          workspace
        </span>
      </div>
      <section aria-label="Box inventory">
        <div className="inventory-toolbar">
          <button
            className="mobile-sort sort-button"
            aria-label="Sort boxes by name"
            onClick={() => setDescending((value) => !value)}
          >
            Name {descending ? <ArrowDown size={14} /> : <ArrowUp size={14} />}
          </button>
          <div className="search-field">
            <Search size={17} aria-hidden="true" />
            <input
              type="search"
              aria-label="Search boxes"
              placeholder="Search boxes…"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
            {search && (
              <button
                className="icon-button"
                aria-label="Clear search"
                onClick={() => setSearch('')}
              >
                <X size={14} />
              </button>
            )}
            <kbd aria-hidden="true">⌕</kbd>
          </div>
          <div className="filters" aria-label="Filter by state">
            {(
              [
                'all',
                ...boxStates.filter(
                  (state) =>
                    ['running', 'paused', 'stopped', 'failed'].includes(state) ||
                    data.boxes.some((box) => box.state === state),
                ),
              ] as const
            ).map((state) => {
              const count =
                state === 'all'
                  ? data.boxes.length
                  : data.boxes.filter((box) => box.state === state).length
              return (
                <button
                  key={state}
                  aria-pressed={filter === state}
                  className={`filter ${filter === state ? 'selected' : ''}`}
                  onClick={() => setFilter(state)}
                >
                  {state[0].toUpperCase() + state.slice(1)} <span>{count}</span>
                </button>
              )
            })}
          </div>
        </div>
        {data.boxes.length === 0 ? (
          <Empty title="No boxes yet" action={<CreateButton onClick={() => setCreating(true)} />}>
            Create a box from a prepared template.
          </Empty>
        ) : boxes.length === 0 ? (
          <Empty
            title="No matching boxes"
            action={
              <button className="button" onClick={reset}>
                Clear filters
              </button>
            }
          >
            Try another name, box ID or state.
          </Empty>
        ) : (
          <div className="table-scroll">
            <table className="box-table" role="table">
              <caption className="sr-only">Boxes and their observed state</caption>
              <thead role="rowgroup">
                <tr role="row">
                  <th
                    role="columnheader"
                    scope="col"
                    aria-label="Name"
                    aria-sort={descending ? 'descending' : 'ascending'}
                  >
                    <button
                      className="sort-button"
                      onClick={() => setDescending((value) => !value)}
                    >
                      Name {descending ? <ArrowDown size={12} /> : <ArrowUp size={12} />}
                    </button>
                  </th>
                  <th role="columnheader" scope="col">
                    State
                  </th>
                  <th role="columnheader" scope="col">
                    Allocation
                  </th>
                  <th role="columnheader" scope="col">
                    Last operation
                  </th>
                  <th role="columnheader" scope="col">
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody role="rowgroup">
                {boxes.map((box) => (
                  <tr key={box.id} role="row">
                    <td role="cell" className="name-cell">
                      <div className="box-name-cell">
                        <div className="box-glyph">
                          <Cpu size={17} strokeWidth={1.4} />
                        </div>
                        <div>
                          <AppLink
                            className="resource-name"
                            to={`/boxes/${encodeURIComponent(box.id)}`}
                          >
                            {box.name}
                          </AppLink>
                          <span className="resource-id">{box.id}</span>
                        </div>
                      </div>
                    </td>
                    <td role="cell" className="state-cell">
                      <Status state={box.state} />
                    </td>
                    <td role="cell" className="allocation-cell">
                      <div className="resource-allocation">
                        {box.resources ? (
                          <>
                            <span>
                              {box.resources.vcpus} <small>vCPU</small>
                            </span>
                            <span>
                              {formatBytes(box.resources.memory_bytes)} <small>RAM</small>
                            </span>
                          </>
                        ) : (
                          '—'
                        )}
                      </div>
                      <span className="resource-id">
                        {box.resources
                          ? box.metrics.disk_bytes !== null
                            ? `${formatDiskUsedOfTotal(box.metrics.disk_bytes, box.resources.disk_bytes)} disk · host-observed`
                            : `${formatBytes(box.resources.disk_bytes)} disk`
                          : 'Not allocated'}
                      </span>
                    </td>
                    <td role="cell" className="event-cell">
                      <span className="operation-text">
                        {box.last_operation ?? 'No operations reported'}
                      </span>
                      <span className="resource-id">
                        {box.state === 'paused'
                          ? 'RAM retained'
                          : box.state === 'stopped'
                            ? 'Disk retained'
                            : box.state === 'failed'
                              ? 'Needs attention'
                              : 'Observed by host'}
                      </span>
                    </td>
                    <td role="cell" className="action-cell">
                      <AppLink
                        className="table-action"
                        aria-label={`Open ${box.name}`}
                        to={`/boxes/${encodeURIComponent(box.id)}${box.state === 'failed' ? '?tab=activity' : ''}`}
                      >
                        <span>{box.state === 'failed' ? 'View error' : 'Open'}</span>
                        <ArrowUpRight size={15} />
                      </AppLink>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="table-footer">
          <span>
            {boxes.length} of {data.boxes.length} boxes
          </span>
          <span>
            <span className="status-dot" /> Observed state · refresh to update
          </span>
        </div>
      </section>
      {failed && (
        <div className="failure-strip">
          <CircleAlert size={19} />
          <div>
            <strong>
              {failed.box_name} <span className="muted">/</span> {failed.kind} failed
            </strong>
            <p>{failed.detail}</p>
          </div>
          <AppLink
            className="text-action"
            to={`/operations?operation=${encodeURIComponent(failed.id)}`}
          >
            View details <ArrowUpRight size={15} />
          </AppLink>
        </div>
      )}
      <ResourceMetrics metrics={data.host.metrics} title="Host metrics" />
      <div className="bottom-note">
        <span className="micro">PRIVATE BY DEFAULT</span>
        <p>Isolated environments. Persistent disks. Your infrastructure.</p>
      </div>
      {creating && (
        <CreateDrawer
          data={data}
          authenticated={data.host.state === 'online'}
          onClose={() => setCreating(false)}
        />
      )}
    </>
  )
}
