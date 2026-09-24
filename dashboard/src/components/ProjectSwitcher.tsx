import { Icon } from './Icons'
import type { ProjectSummary } from '../api/types'

interface ProjectSwitcherProps {
  projects: ProjectSummary[]
  selectedProject: string
  onChange: (projectName: string) => void
  mode: 'preview' | 'api'
}

export function ProjectSwitcher({
  projects,
  selectedProject,
  onChange,
  mode,
}: ProjectSwitcherProps) {
  const options = projects.some((project) => project.name === selectedProject)
    ? projects
    : [
        {
          name: selectedProject,
          root: 'Current project',
          lastActiveAt: new Date().toISOString(),
        },
        ...projects,
      ]

  return (
    <div className="project-switcher">
      <span className="project-switcher__label">Active project</span>
      <div className="select-control select-control--project">
        <Icon name="box" size={16} />
        <select
          aria-label="Active project"
          onChange={(event) => onChange(event.target.value)}
          value={selectedProject}
        >
          {options.map((project) => (
            <option key={project.name} value={project.name}>
              {project.name}
            </option>
          ))}
        </select>
        <Icon name="chevron-down" size={14} />
      </div>
      <span className="project-switcher__note">
        <span className="project-switcher__dot" />
        {mode === 'preview' ? 'Preview dataset' : 'Project API placeholder'}
      </span>
    </div>
  )
}
