import { useState, type ReactNode } from 'react'
import { Icon, StrataMark, type IconName } from './Icons'
import { ProjectSwitcher } from './ProjectSwitcher'
import { StatusDot } from './StateViews'
import type { ProjectSummary } from '../api/types'

export type DashboardView = 'overview' | 'memory' | 'graph' | 'retrieval' | 'operations'

interface AppShellProps {
  project: string
  projects: ProjectSummary[]
  activeView: DashboardView
  mode: 'preview' | 'api'
  connectionState: 'healthy' | 'degraded' | 'offline'
  onNavigate: (view: DashboardView) => void
  onProjectChange: (projectName: string) => void
  onRefresh: () => void
  children: ReactNode
}

interface NavItem {
  id: DashboardView
  label: string
  hint: string
  icon: IconName
}

const navItems: NavItem[] = [
  { id: 'overview', label: 'Overview', hint: 'Pulse', icon: 'grid' },
  { id: 'memory', label: 'Memory explorer', hint: 'Browse', icon: 'memory' },
  { id: 'graph', label: 'Entity graph', hint: 'Connect', icon: 'graph' },
  { id: 'retrieval', label: 'Retrieval lab', hint: 'Tune', icon: 'flask' },
  { id: 'operations', label: 'Operations', hint: 'Health', icon: 'activity' },
]

export function AppShell({
  project,
  projects,
  activeView,
  mode,
  connectionState,
  onNavigate,
  onProjectChange,
  onRefresh,
  children,
}: AppShellProps) {
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const activeItem = navItems.find((item) => item.id === activeView) ?? navItems[0]

  const navigate = (view: DashboardView) => {
    onNavigate(view)
    setSidebarOpen(false)
  }

  return (
    <div className="app-shell">
      <button
        aria-label="Close navigation"
        className={`sidebar-scrim ${sidebarOpen ? 'sidebar-scrim--visible' : ''}`}
        onClick={() => setSidebarOpen(false)}
        type="button"
      />
      <aside className={`sidebar ${sidebarOpen ? 'sidebar--open' : ''}`}>
        <div className="brand-lockup">
          <StrataMark size={30} />
          <div>
            <strong>statefulmemory</strong>
            <span>local workspace</span>
          </div>
        </div>

        <ProjectSwitcher
          mode={mode}
          onChange={onProjectChange}
          projects={projects}
          selectedProject={project}
        />

        <nav aria-label="Dashboard sections" className="primary-nav">
          <span className="nav-section-label">Workspace</span>
          {navItems.map((item) => (
            <button
              aria-current={activeView === item.id ? 'page' : undefined}
              className={`nav-item ${activeView === item.id ? 'nav-item--active' : ''}`}
              key={item.id}
              onClick={() => navigate(item.id)}
              type="button"
            >
              <span className="nav-item__icon">
                <Icon name={item.icon} size={17} />
              </span>
              <span className="nav-item__copy">
                <strong>{item.label}</strong>
                <small>{item.hint}</small>
              </span>
              {item.id === 'operations' && connectionState !== 'healthy' ? (
                <span className="nav-item__alert" />
              ) : null}
            </button>
          ))}
        </nav>

        <div className="sidebar__footer">
          <div className="sidebar-health">
            <StatusDot tone={connectionState === 'healthy' ? 'healthy' : 'warning'} />
            <div>
              <strong>{connectionState === 'healthy' ? 'Local daemon online' : 'Daemon needs attention'}</strong>
              <span>uds://127.0.0.1/statefulmemory.sock</span>
            </div>
            <button aria-label="Refresh daemon status" className="icon-button icon-button--small" onClick={onRefresh} type="button">
              <Icon name="refresh" size={14} />
            </button>
          </div>
          <span className="sidebar-version">dashboard shell · v0.1</span>
        </div>
      </aside>

      <div className="main-shell">
        <header className="topbar">
          <div className="topbar__leading">
            <button
              aria-label="Open navigation"
              className="mobile-menu-button icon-button"
              onClick={() => setSidebarOpen(true)}
              type="button"
            >
              <Icon name="menu" size={19} />
            </button>
            <div className="breadcrumb">
              <span>Workspace</span>
              <Icon name="chevron-right" size={13} />
              <strong>{activeItem.label}</strong>
            </div>
          </div>
          <div className="topbar__actions">
            <button className="command-trigger" type="button">
              <Icon name="command" size={14} />
              <span>Jump to a view</span>
              <kbd>⌘ K</kbd>
            </button>
            <button aria-label="More workspace actions" className="icon-button" type="button">
              <Icon name="more" size={18} />
            </button>
            <span className="topbar-avatar">SM</span>
          </div>
        </header>
        <main className="main-content">{children}</main>
      </div>
    </div>
  )
}
