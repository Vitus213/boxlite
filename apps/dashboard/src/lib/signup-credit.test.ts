import { afterEach, expect, it, vi } from 'vitest'
import { waitForSignupCredit } from './signup-credit'
afterEach(() => vi.useRealTimers())
it('waits through pending reads and only resolves after delivery', async () => {
  vi.useFakeTimers()
  const read = vi
    .fn()
    .mockResolvedValueOnce({ delivered: false })
    .mockResolvedValueOnce({ delivered: false })
    .mockResolvedValue({ delivered: true })
  const finished = vi.fn()
  const result = waitForSignupCredit(read, new AbortController().signal).then(finished)
  await vi.advanceTimersByTimeAsync(5999)
  expect(read).toHaveBeenCalledTimes(2)
  expect(finished).not.toHaveBeenCalled()
  await vi.advanceTimersByTimeAsync(1)
  await result
  expect(read).toHaveBeenCalledTimes(3)
  expect(finished).toHaveBeenCalledTimes(1)
})
it('exhaustion remains an error, never a successful grant', async () => {
  vi.useFakeTimers()
  const read = vi.fn().mockResolvedValue({ delivered: false })
  const result = waitForSignupCredit(read, new AbortController().signal)
  const assertion = expect(result).rejects.toThrow('Credits have not arrived yet')
  await vi.runAllTimersAsync()
  await assertion
  expect(read).toHaveBeenCalledTimes(8)
})
it('stops requests on cancellation', async () => {
  vi.useFakeTimers()
  const read = vi.fn().mockResolvedValue({ delivered: false })
  const controller = new AbortController()
  const result = waitForSignupCredit(read, controller.signal)
  const assertion = expect(result).rejects.toThrow()
  await vi.advanceTimersByTimeAsync(1)
  controller.abort()
  await assertion
  await vi.runAllTimersAsync()
  expect(read).toHaveBeenCalledTimes(1)
})
it('propagates request errors without confirming credit', async () => {
  await expect(
    waitForSignupCredit(() => Promise.reject(new Error('unavailable')), new AbortController().signal),
  ).rejects.toThrow('unavailable')
})
