import { http, HttpResponse } from 'msw'
import { beforeEach, describe, expect, it } from 'vitest'

import { createAndOpenProject, reopenLastProject, switchProject } from '@/lib/io/scene'
import { useEditorUiStore } from '@/lib/stores/editorUiStore'

import { server } from '../../msw/server'

const summary = (id: string) => ({ id, name: id, path: `/projects/${id}.khrproj` })

let opened: string[]
let existing: string[]

beforeEach(() => {
  opened = []
  existing = ['badend', '926']
  useEditorUiStore.setState({ lastProjectId: undefined })
  server.use(
    http.get('/api/v1/projects', () => HttpResponse.json({ projects: existing.map(summary) })),
    http.put('/api/v1/projects/current', async ({ request }) => {
      const { id } = (await request.json()) as { id: string }
      opened.push(id)
      return HttpResponse.json(summary(id))
    }),
    http.post('/api/v1/projects', () => HttpResponse.json(summary('new-one'))),
  )
})

describe('the last project (mouse forward button)', () => {
  it('is remembered when a project is opened or created', async () => {
    await switchProject({ id: 'badend' })
    expect(useEditorUiStore.getState().lastProjectId).toBe('badend')

    await createAndOpenProject({ name: 'new one' })
    expect(useEditorUiStore.getState().lastProjectId).toBe('new-one')
  })

  it('reopens the last project', async () => {
    useEditorUiStore.setState({ lastProjectId: '926' })

    await expect(reopenLastProject()).resolves.toBe(true)
    expect(opened).toEqual(['926'])
  })

  it('does nothing when there is none or it was deleted', async () => {
    await expect(reopenLastProject()).resolves.toBe(false)

    useEditorUiStore.setState({ lastProjectId: 'gone' })
    await expect(reopenLastProject()).resolves.toBe(false)
    expect(opened).toEqual([])
  })

  it('opens it once when the button is pressed twice quickly', async () => {
    useEditorUiStore.setState({ lastProjectId: 'badend' })

    const results = await Promise.all([reopenLastProject(), reopenLastProject()])

    expect(results.sort()).toEqual([false, true])
    expect(opened).toEqual(['badend'])
  })
})
