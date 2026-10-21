import { describe, expect, it, vi } from 'vitest'
import { releaseNotesEmptyMessage, releaseNotesFor } from './release-notes'

describe('releaseNotesFor', () => {
  it('returns embedded notes for the build version', () => {
    const notes = releaseNotesFor(__KILN_BUILD_VERSION__)
    expect(notes?.title).toMatch(/^v/)
    expect(notes?.changes.length).toBeGreaterThan(0)
  })

  it('returns null when the running version does not match the build', () => {
    expect(releaseNotesFor('9.9.9')).toBeNull()
  })
})

describe('releaseNotesEmptyMessage', () => {
  it('names the requested version', () => {
    expect(releaseNotesEmptyMessage('1.2.3')).toBe('No release notes for v1.2.3')
  })
})
