import { expect, it, vi } from 'vitest'

import { applyCommand } from '@/lib/api/default/default'
import { applyOp, awaitPendingSceneEdits } from '@/lib/io/scene'

vi.mock('@/lib/api/default/default', () => ({
  applyCommand: vi.fn().mockResolvedValue({ epoch: 1 }),
  getGetSceneJsonQueryKey: () => ['scene'],
}))
vi.mock('@/lib/queryClient', () => ({
  queryClient: { invalidateQueries: vi.fn().mockResolvedValue(undefined) },
}))

it('waits for queued deletion before a pipeline can read its masks', async () => {
  let release!: () => void
  const held = new Promise<void>((resolve) => {
    release = resolve
  })
  vi.mocked(applyCommand).mockImplementationOnce(async () => {
    await held
    return { epoch: 1 }
  })
  const save = applyOp({ batch: { ops: [], label: 'delete' } })
  let started = false
  const pipeline = awaitPendingSceneEdits().then(() => {
    started = true
  })
  await Promise.resolve()
  expect(started).toBe(false)
  release()
  await Promise.all([save, pipeline])
  expect(started).toBe(true)
})
