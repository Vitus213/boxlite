/* Copyright 2026 BoxLite AI · SPDX-License-Identifier: AGPL-3.0 */
/** Match #2125's bounded backoff; exhaustion must never imply delivery. */
export async function waitForSignupCredit(
  read: () => Promise<{ delivered: boolean }>,
  signal: AbortSignal,
): Promise<void> {
  for (let attempt = 0; attempt < 8; attempt++) {
    signal.throwIfAborted()
    const delivery = await read()
    signal.throwIfAborted()
    if (delivery.delivered === true) return
    if (attempt === 7) break
    await new Promise<void>((resolve, reject) => {
      const abort = () => {
        clearTimeout(timer)
        reject(signal.reason)
      }
      const timer = setTimeout(
        () => {
          signal.removeEventListener('abort', abort)
          resolve()
        },
        Math.min(30_000, 2000 * 2 ** attempt),
      )
      signal.addEventListener('abort', abort, { once: true })
      if (signal.aborted) abort()
    })
  }
  throw new Error(
    'Credits have not arrived yet. Check that your email is verified and this organization is eligible, then try again.',
  )
}
