import { HttpDashboardApi } from './http-client'
import { PreviewDashboardApi } from '../data/preview'
import type { DashboardApi } from './types'

export function createDashboardApi(): DashboardApi {
  const source =
    import.meta.env.VITE_DASHBOARD_DATA_SOURCE ?? (import.meta.env.DEV ? 'preview' : 'api')
  if (source === 'api') {
    return new HttpDashboardApi(import.meta.env.VITE_DASHBOARD_API_URL ?? '')
  }
  return new PreviewDashboardApi()
}

export { DashboardApiError, HttpDashboardApi } from './http-client'
export { PreviewDashboardApi } from '../data/preview'
export type * from './types'
