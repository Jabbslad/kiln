import { ExternalLink } from 'lucide-react'
import { kilnVersionLabel } from './kiln-version'
import { releaseNotesEmptyMessage, releaseNotesFor } from './release-notes'
import { Modal } from './ui'

export function ReleaseNotesDialog({
  displayVersion,
  onClose,
}: {
  displayVersion?: string
  onClose: () => void
}) {
  const label = kilnVersionLabel(displayVersion)
  const notes = releaseNotesFor(displayVersion)
  return (
    <Modal title={notes?.title ?? label} onClose={onClose}>
      <div className="modal-body release-notes-body">
        {notes && notes.changes.length > 0 ? (
          <>
            <ul className="release-notes-list">
              {notes.changes.map((change) => (
                <li key={change.pr ?? change.summary}>
                  {change.summary}
                  {change.pr ? (
                    <>
                      {' '}
                      <span className="release-notes-pr">#{change.pr}</span>
                    </>
                  ) : null}
                </li>
              ))}
            </ul>
            <p className="release-notes-link">
              <a href={notes.releaseUrl} target="_blank" rel="noreferrer">
                View on GitHub
                <ExternalLink size={14} aria-hidden="true" />
              </a>
            </p>
          </>
        ) : (
          <p className="release-notes-empty">{releaseNotesEmptyMessage(displayVersion)}</p>
        )}
      </div>
    </Modal>
  )
}
