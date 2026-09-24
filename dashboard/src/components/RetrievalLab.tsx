import { useState, type FormEvent } from 'react'
import type { DashboardApi, RetrievalExplanation } from '../api/types'
import { Icon } from './Icons'
import { StatusPill } from './StateViews'

interface RetrievalLabProps {
  projectName: string
  api: DashboardApi
}

const pipeline = [
  { label: 'BM25', detail: 'lexical recall', state: 'ready' },
  { label: 'Dense', detail: 'BGE-small', state: 'ready' },
  { label: 'RRF', detail: 'rank fusion', state: 'ready' },
  { label: 'Rerank', detail: 'local CE', state: 'ready' },
]

export function RetrievalLab({ projectName, api }: RetrievalLabProps) {
  const [query, setQuery] = useState('')
  const [mode, setMode] = useState('hybrid')
  const [topK, setTopK] = useState('15')
  const [explanation, setExplanation] = useState<RetrievalExplanation | null>(null)
  const [status, setStatus] = useState<'idle' | 'loading' | 'success' | 'error'>('idle')
  const [error, setError] = useState<Error | null>(null)

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const normalized = query.trim()
    if (!normalized) return
    setStatus('loading')
    setError(null)
    try {
      setExplanation(await api.explainRetrieval(projectName, normalized, Number(topK), mode))
      setStatus('success')
    } catch (reason) {
      setError(reason instanceof Error ? reason : new Error('Retrieval trace failed'))
      setStatus('error')
    }
  }

  return (
    <div className="placeholder-page">
      <div className="page-heading">
        <div>
          <div className="eyebrow"><Icon name="flask" size={13} /> Retrieval workbench</div>
          <h1>Retrieval lab</h1>
          <p>Inspect how local recall, fusion, and reranking shape a context window in {projectName}.</p>
        </div>
        <StatusPill icon={status === 'error' ? 'warning' : 'spark'} tone={status === 'error' ? 'warning' : 'copper'}>
          {status === 'error' ? 'Error' : 'Local trace'}
        </StatusPill>
      </div>

      <div className="retrieval-layout">
        <section className="panel retrieval-form-panel">
          <div className="panel__header">
            <div><h2>Query composer</h2><p>Start with a question your agents might ask.</p></div>
            <span className="mono-label">POST /api/retrieval/explain</span>
          </div>
          <form className="retrieval-form" onSubmit={handleSubmit}>
            <label className="field-label">Query
              <textarea value={query} onChange={(event) => setQuery(event.target.value)} placeholder="What decision shaped the write thread?" rows={5} />
            </label>
            <div className="retrieval-form__row">
              <label className="field-label">Mode
                <span className="select-control select-control--field">
                  <select value={mode} onChange={(event) => setMode(event.target.value)}>
                    <option value="hybrid">Hybrid</option>
                    <option value="bm25">BM25 only</option>
                  </select>
                  <Icon name="chevron-down" size={13} />
                </span>
              </label>
              <label className="field-label">Top K
                <span className="select-control select-control--field">
                  <select value={topK} onChange={(event) => setTopK(event.target.value)}>
                    <option value="8">8 results</option>
                    <option value="15">15 results</option>
                    <option value="30">30 results</option>
                  </select>
                  <Icon name="chevron-down" size={13} />
                </span>
              </label>
            </div>
            <div className="retrieval-form__footer">
              <span><Icon name="shield" size={14} /> Local retrieval · no third-party key</span>
              <button className="button button--primary" type="submit" disabled={status === 'loading'}>
                <Icon name="play" size={15} />{status === 'loading' ? 'Tracing…' : 'Run trace'}
              </button>
            </div>
          </form>
        </section>

        <section className="panel pipeline-panel">
          <div className="panel__header">
            <div><h2>Retrieval pipeline</h2><p>Current local configuration</p></div>
            <span className="pipeline-health"><span className="status-dot status-dot--healthy" /> all stages ready</span>
          </div>
          <div className="pipeline-list">
            {pipeline.map((stage, index) => (
              <div className="pipeline-stage" key={stage.label}>
                <span className="pipeline-stage__index">0{index + 1}</span>
                <span className="pipeline-stage__copy"><strong>{stage.label}</strong><small>{stage.detail}</small></span>
                <Icon name="check" size={16} />
              </div>
            ))}
          </div>
          <div className="pipeline-note"><Icon name="info" size={15} /><span>Reranker stays local by default. LLM-backed synthesis remains opt-in.</span></div>
        </section>
      </div>

      <section className={`panel retrieval-result ${explanation ? 'retrieval-result--active' : ''}`}>
        <div className="panel__header">
          <div><h2>Result trace</h2><p>{explanation ? `Trace for “${explanation.query}”` : 'Run a query to inspect candidate provenance.'}</p></div>
          <StatusPill tone={explanation ? 'copper' : 'neutral'}>{explanation ? `${explanation.elapsedUs}µs` : 'Waiting'}</StatusPill>
        </div>
        {error ? <p className="inline-error">{error.message}</p> : null}
        {explanation ? (
          <div className="result-trace-grid">
            <div className="trace-slot"><span>01 · Recall</span><strong>{explanation.candidateDepth} candidates</strong><small>{explanation.mode} search</small></div>
            <div className="trace-slot"><span>02 · Fuse</span><strong>RRF rank</strong><small>{explanation.candidates.filter((candidate) => candidate.sources.length > 1).length} dual-source</small></div>
            <div className="trace-slot"><span>03 · Rerank</span><strong>{explanation.rerank || 'none'}</strong><small>{explanation.rerankTimedOut ? 'timed out; fallback used' : 'within budget'}</small></div>
            <div className="trace-slot trace-slot--accent"><span>04 · Response</span><strong>{explanation.results.length} results</strong><small>{explanation.rerankTimedOut ? 'fallback order' : 'ranked context'}</small></div>
          </div>
        ) : (
          <div className="result-empty"><Icon name="flask" size={22} /><span>Candidate ranks, sources, and timings appear here after a trace.</span></div>
        )}
      </section>
    </div>
  )
}
