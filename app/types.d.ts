export interface Connection {
  address: string
  heapSize: string
  id: string
  javaHome: string
  javaArgs: string
  name: string
  username: string
  password: string
  group: string
  notes: string
  donotcache: boolean
  lastConnected: number | null
  showConsole: boolean
  engineType: string
  pinnedCertSha256: string | null
}

// Server leaf certificate details shown in the trust prompt. `sha256` (hex) is
// the value to verify out-of-band; the rest is self-asserted context.
export interface CertInfo {
  sha256: string
  subject: string
  issuer: string
  not_after: string
}

