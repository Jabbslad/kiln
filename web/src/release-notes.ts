import { z } from 'zod'
import { kilnVersionLabel } from './kiln-version'
import embedded from './release-notes.json'

export const releaseNotesSchema = z.object({
  version: z.string().min(1),
  title: z.string().min(1),
  changes: z.array(
    z.object({
      summary: z.string().min(1),
      pr: z.number().int().positive().optional(),
    }),
  ),
  releaseUrl: z.string().url(),
})

export type ReleaseNotes = z.infer<typeof releaseNotesSchema>

const buildNotes = releaseNotesSchema.parse(embedded)

export function releaseNotesFor(displayVersion?: string): ReleaseNotes | null {
  const raw = (displayVersion ?? __KILN_BUILD_VERSION__).replace(/^v/, '')
  if (buildNotes.version !== raw) return null
  return buildNotes
}

export function releaseNotesEmptyMessage(displayVersion?: string) {
  return `No release notes for ${kilnVersionLabel(displayVersion)}`
}
