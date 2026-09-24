import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it } from 'vitest'
import { App } from './App'
import { PreviewDashboardApi } from './data/preview'

describe('dashboard shell', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', '/')
  })

  it('renders the local-first overview and opens a memory drawer', async () => {
    render(<App api={new PreviewDashboardApi(0)} />)

    expect(await screen.findByRole('heading', { name: 'Memory, made legible.' })).toBeInTheDocument()
    expect(screen.getByText('Recent memories')).toBeInTheDocument()

    await userEvent.click(screen.getByRole('button', { name: /Keep database writes on the project write thread/ }))

    expect(screen.getByRole('dialog', { name: 'Memory details' })).toBeInTheDocument()
    expect(screen.getByText('Record timeline')).toBeInTheDocument()
  })

  it('keeps project selection in the URL', async () => {
    const user = userEvent.setup()
    render(<App api={new PreviewDashboardApi(0)} />)

    await screen.findByRole('heading', { name: 'Memory, made legible.' })
    await user.selectOptions(screen.getByRole('combobox', { name: 'Active project' }), 'atlas-api')

    await waitFor(() => {
      expect(new URLSearchParams(window.location.search).get('project')).toBe('atlas-api')
    })
    expect(await screen.findByText('Paginate list endpoints with opaque cursors')).toBeInTheDocument()
  })
})
