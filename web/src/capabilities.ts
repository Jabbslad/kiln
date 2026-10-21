export type CapabilityStatus = 'available' | 'not-yet'

export type Capability =
  'metrics' | 'terminal' | 'ssh' | 'httpPreview' | 'publicUrl' | 'launchTiming'

export const capabilities = {
  metrics: 'available',
  terminal: 'available',
  ssh: 'not-yet',
  httpPreview: 'not-yet',
  publicUrl: 'not-yet',
  launchTiming: 'not-yet',
} as const satisfies Record<Capability, CapabilityStatus>

export function capabilityAvailable(capability: Capability): boolean {
  return (capabilities[capability] as CapabilityStatus) === 'available'
}
