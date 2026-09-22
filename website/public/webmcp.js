// WebMCP — expose statefulmemory docs actions to in-browser AI agents.
// Spec: https://webmachinelearning.github.io/webmcp/
// Registered on page load; no-ops when the browser lacks navigator.modelContext.
(function () {
  const mc = navigator.modelContext;
  if (!mc || typeof mc.registerTool !== 'function') return;

  const origin = window.location.origin;
  const controller = new AbortController();

  const tools = [
    {
      name: 'search_docs',
      description:
        'Search the statefulmemory documentation for a topic (install, config, agents, graph, mem, decide, self-hosting).',
      inputSchema: {
        type: 'object',
        properties: {
          query: { type: 'string', description: 'Search terms, e.g. "anchor verify" or "team token".' },
        },
        required: ['query'],
      },
      execute: async ({ query }) => ({
        url: `${origin}/docs/?q=${encodeURIComponent(query)}`,
        text: `Search results for "${query}" in the statefulmemory docs.`,
      }),
    },
    {
      name: 'open_docs_page',
      description: 'Open a specific statefulmemory documentation page by slug.',
      inputSchema: {
        type: 'object',
        properties: {
          slug: {
            type: 'string',
            description:
              'Docs slug without leading slash, e.g. "docs/install/", "docs/commands/", "docs/agents/".',
          },
        },
        required: ['slug'],
      },
      execute: async ({ slug }) => {
        const clean = String(slug).replace(/^\/+/, '');
        return { url: `${origin}/${clean}`, text: `Opened ${clean}.` };
      },
    },
    {
      name: 'get_install_command',
      description: 'Return the statefulmemory install and wiring commands for a given coding agent.',
      inputSchema: {
        type: 'object',
        properties: {
          agent: {
            type: 'string',
            description: 'Agent id, e.g. cursor, opencode, codex, gemini, claude-code. Omit to auto-detect.',
          },
        },
      },
      execute: async ({ agent } = {}) => {
        const cmd = agent
          ? `statefulmemory install --agent ${agent}`
          : 'statefulmemory install';
        return { command: cmd, docs: `${origin}/docs/install/`, text: `Run: ${cmd}` };
      },
    },
  ];

  for (const tool of tools) {
    try {
      mc.registerTool(tool, { signal: controller.signal });
    } catch {
      try {
        mc.registerTool(tool);
      } catch {
        /* ignore unsupported registration shapes */
      }
    }
  }
})();
