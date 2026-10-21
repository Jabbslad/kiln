/** Canonical operation action tags; must match kiln_api::Action serde `type` values. */
export const OPERATION_ACTION_TYPES = [
  'create',
  'pause',
  'resume',
  'stop',
  'start',
  'delete',
  'exec',
  'checkpoint',
  'restore',
] as const

export type OperationActionType = (typeof OPERATION_ACTION_TYPES)[number]
