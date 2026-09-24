import { useEffect, useMemo, useState, type FormEvent } from 'react'
import type { DashboardApi, GraphResponse, GraphStats } from '../api/types'
import { Icon } from './Icons'
import { StatusPill } from './StateViews'

interface GraphPlaceholderProps {
  projectName: string
  api: DashboardApi
}

function positionNodes(graph: GraphResponse) {
  const entities = graph.entities.slice(0, 64)
  const center = entities.find((entity) => entity.id === (graph.seedId ?? graph.seed)) ?? entities[0]
  const positions = new Map<string, { x: number; y: number }>()
  if (!center) return positions
  positions.set(center.id, { x: 50, y: 50 })
  const ring = entities.filter((entity) => entity.id !== center.id)
  ring.forEach((entity, index) => {
    const angle = (Math.PI * 2 * index) / Math.max(1, ring.length) - Math.PI / 2
    positions.set(entity.id, {
      x: 50 + Math.cos(angle) * 34,
      y: 50 + Math.sin(angle) * 34,
    })
  })
  return positions
}

export function GraphPlaceholder({ projectName, api }: GraphPlaceholderProps) {
  const [entity, setEntity] = useState('memory')
  const [hops, setHops] = useState(1)
  const [graph, setGraph] = useState<GraphResponse>({ seed: '', entities: [], edges: [] })
  const [stats, setStats] = useState<GraphStats | null>(null)
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading')
  const [error, setError] = useState<Error | null>(null)
  const positions = useMemo(() => positionNodes(graph), [graph])

  useEffect(() => {
    let active = true
    setStatus('loading')
    setError(null)
    void api.getGraphStats(projectName).then((value) => {
      if (!active) return
      setStats(value)
      setStatus('ready')
    }, (reason: unknown) => {
      if (!active) return
      setError(reason instanceof Error ? reason : new Error('Graph data unavailable'))
      setStatus('error')
    })
    return () => {
      active = false
    }
  }, [api, projectName])

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const query = entity.trim()
    if (!query) return
    setStatus('loading')
    setError(null)
    try {
      setGraph(await api.getGraph(projectName, query, hops))
      setStatus('ready')
    } catch (reason) {
      setError(reason instanceof Error ? reason : new Error('Graph query failed'))
      setStatus('error')
    }
  }

  return (
    <div className="placeholder-page">
      <div className="page-heading">
        <div>
          <div className="eyebrow"><Icon name="graph" size={13} /> Relationship layer</div>
          <h1>Entity graph</h1>
          <p>Trace files, symbols, agents, and concepts connected in {projectName}.</p>
        </div>
        <StatusPill icon={status === 'error' ? 'warning' : 'spark'} tone={status === 'error' ? 'warning' : 'copper'}>
          {status === 'error' ? 'Unavailable' : 'Local graph'}
        </StatusPill>
      </div>

      <div className="graph-layout">
        <section className="panel graph-panel">
          <div className="panel__header">
            <div>
              <h2>Neighborhood explorer</h2>
              <p>Query a seed entity and inspect its weighted neighborhood.</p>
            </div>
            <span className="graph-mode"><span className={`status-dot status-dot--${status === 'error' ? 'warning' : 'healthy'}`} /> {status === 'loading' ? 'Loading' : 'PPR'}</span>
          </div>
          <form className="graph-controls" onSubmit={handleSubmit}>
            <label className="search-field graph-search">
              <Icon name="target" size={16} />
              <span className="sr-only">Graph seed entity</span>
              <input value={entity} onChange={(event) => setEntity(event.target.value)} placeholder="Entity name" />
            </label>
            <label className="hops-field">
              <span>hops</span>
              <select value={hops} onChange={(event) => setHops(Number(event.target.value))}>
                <option value={1}>1 hop</option>
                <option value={2}>2 hops</option>
              </select>
              <Icon name="chevron-down" size={13} />
            </label>
            <button className="button button--primary" type="submit" disabled={status === 'loading'}>
              {status === 'loading' ? 'Loading…' : 'Explore graph'}
            </button>
          </form>
          {error ? <p className="inline-error">{error.message}</p> : null}
          <div className="graph-canvas-wrap">
            <div className="graph-canvas__topline">
              <span>Seed <strong>{graph.seed || entity || '—'}</strong></span>
              <span>{graph.entities.length} nodes · {graph.edges.length} edges</span>
            </div>
            <div className="graph-canvas" role="img" aria-label={`Entity graph for ${graph.seed || entity}`}>
              {graph.entities.length ? (
                <svg viewBox="0 0 100 100" preserveAspectRatio="none">
                  <defs>
                    <linearGradient id="graphEdge" x1="0" x2="1" y1="0" y2="1">
                      <stop offset="0" stopColor="var(--color-copper)" stopOpacity="0.6" />
                      <stop offset="1" stopColor="var(--color-copper)" stopOpacity="0.12" />
                    </linearGradient>
                  </defs>
                  {graph.edges.map((edge, index) => {
                    const source = positions.get(edge.source)
                    const target = positions.get(edge.target)
                    if (!source || !target) return null
                    return <line key={`${edge.source}-${edge.target}-${index}`} className="graph-edge" x1={source.x} y1={source.y} x2={target.x} y2={target.y} strokeWidth={Math.min(1 + edge.weight, 4)} />
                  })}
                  {graph.entities.map((item) => {
                    const position = positions.get(item.id)
                    if (!position) return null
                    const isSeed = item.id === (graph.seedId ?? graph.seed)
                    return (
                      <g key={item.id}>
                        <circle className={`graph-node-halo graph-node-halo--${item.kind}`} cx={position.x} cy={position.y} r={isSeed ? 20 : 14} />
                        <circle className="graph-node" cx={position.x} cy={position.y} r={isSeed ? 11 : 7} />
                        <text className="graph-node-label" textAnchor="middle" x={position.x} y={position.y + 1}>{item.name.slice(0, 16)}</text>
                      </g>
                    )
                  })}
                </svg>
              ) : (
                <div className="result-empty"><Icon name="graph" size={22} /><span>Enter an entity to load a neighborhood.</span></div>
              )}
              {stats ? <span className="graph-canvas__badge"><Icon name="spark" size={13} /> {Math.round(stats.coverageRatio * 100)}% observation coverage</span> : null}
            </div>
          </div>
        </section>

        <aside className="graph-side-stack">
          <section className="panel graph-inspector">
            <div className="panel__header panel__header--small"><div><h2>Graph index</h2><p>Current coverage</p></div><Icon name="more" size={17} /></div>
            <div className="seed-entity"><span className="seed-entity__mark"><Icon name="target" size={18} /></span><div><strong>{stats?.totalEntities ?? '—'}</strong><span>indexed entities</span></div></div>
            <div className="graph-metrics"><div><strong>{stats?.totalEdges ?? '—'}</strong><span>edges</span></div><div><strong>{stats?.totalMentions ?? '—'}</strong><span>mentions</span></div><div><strong>{stats ? `${Math.round(stats.coverageRatio * 100)}%` : '—'}</strong><span>coverage</span></div></div>
          </section>
          <section className="panel graph-legend">
            <div className="panel__header panel__header--small"><div><h2>Legend</h2><p>Entity kinds</p></div></div>
            <div className="legend-grid">
              <span><i className="legend-swatch legend-swatch--copper" /> Concept</span>
              <span><i className="legend-swatch legend-swatch--blue" /> File</span>
              <span><i className="legend-swatch legend-swatch--green" /> Symbol</span>
              <span><i className="legend-swatch legend-swatch--amber" /> Agent</span>
            </div>
          </section>
        </aside>
      </div>
    </div>
  )
}
