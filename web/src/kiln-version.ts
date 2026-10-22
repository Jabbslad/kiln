export function kilnVersionLabel(serverVersion?: string) {
  const raw = serverVersion ?? __KILN_BUILD_VERSION__
  return raw.startsWith('v') ? raw : `v${raw}`
}
