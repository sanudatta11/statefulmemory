interface ImportMetaEnv {
  readonly VITE_DASHBOARD_DATA_SOURCE?: 'preview' | 'api'
  readonly VITE_DASHBOARD_API_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
