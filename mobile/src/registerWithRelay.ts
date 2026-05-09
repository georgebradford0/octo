// Tunnel-driven push-notification registration.
//
// On every successful chat-screen mount we hit the lair's /info endpoint over
// the encrypted Noise tunnel; lair returns its Ed25519 relay-signing pubkey
// alongside the URL of the relay it pushes through. We then ask iOS for the
// APNs device token (sticky after first grant) and POST it to the relay so
// the relay's table maps (device_token, lair_pubkey). Idempotent — the relay
// upserts on (device_token, lair_pubkey) so re-running on every reconnect
// just touches the row.
//
// All errors are logged and swallowed; missing push capability never breaks
// the rest of the app.

import { Platform } from 'react-native'
import NativePush from './NativePush'

interface LairInfo {
  pubkey:               string
  relay_signing_pubkey?: string
  relay_url?:           string
}

const registered = new Set<string>()

export async function registerWithRelay(baseUrl: string, log: (m: string) => void): Promise<void> {
  if (Platform.OS !== 'ios')         return  // FCM not wired yet
  if (registered.has(baseUrl))       return
  if (!NativePush)                   { log('[push] native module not registered'); return }

  let info: LairInfo
  try {
    const r = await fetch(`${baseUrl}/info`)
    if (!r.ok) { log(`[push] /info HTTP ${r.status}`); return }
    info = await r.json() as LairInfo
  } catch (e) {
    log(`[push] /info fetch failed: ${String(e)}`)
    return
  }

  if (!info.relay_signing_pubkey || !info.relay_url) {
    log('[push] lair did not advertise a relay — skipping')
    return
  }

  let token: string | null
  try {
    token = await NativePush.requestPermissionAndRegister()
  } catch (e) {
    log(`[push] APNs registration failed: ${String(e)}`)
    return
  }
  if (!token) {
    log('[push] notifications declined by user')
    return
  }

  try {
    const res = await fetch(`${info.relay_url.replace(/\/$/, '')}/register`, {
      method:  'POST',
      headers: { 'content-type': 'application/json' },
      body:    JSON.stringify({
        device_token: token,
        platform:     'ios',
        lair_pubkey:  info.relay_signing_pubkey,
      }),
    })
    if (!res.ok) {
      log(`[push] /register HTTP ${res.status}`)
      return
    }
    registered.add(baseUrl)
    log(`[push] registered with relay ${info.relay_url} for pubkey ${info.relay_signing_pubkey}`)
  } catch (e) {
    log(`[push] /register fetch failed: ${String(e)}`)
  }
}
