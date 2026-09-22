// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

const site = 'https://statefulmemory.dev';
const description =
	'Persistent memory for coding agents — self-host or StatefulMemory Cloud SaaS. CLI + MCP, daemon, per-project SQLite.';

// https://astro.build/config
export default defineConfig({
	site,
	base: '/',
	compressHTML: true,
	build: {
		inlineStylesheets: 'auto',
	},
	integrations: [
		starlight({
			title: 'statefulmemory',
			description,
			favicon: '/favicon.svg',
			customCss: ['./src/styles/custom.css'],
			pagefind: true,
			components: {
				Header: './src/components/Header.astro',
				SocialIcons: './src/components/SocialIcons.astro',
			},
			social: [
				{
					icon: 'github',
					label: 'GitHub',
					href: 'https://github.com/sanudatta11/statefulmemory',
				},
			],
			editLink: {
				baseUrl: 'https://github.com/sanudatta11/statefulmemory/edit/main/website/',
			},
			lastUpdated: true,
			pagination: true,
			sidebar: [
				{
					label: 'Start here',
					items: [
						{ label: 'Overview', slug: '' },
						{ label: 'Why statefulmemory', slug: 'docs/why-statefulmemory' },
						{ label: 'Architecture', slug: 'docs/architecture' },
						{ label: 'Install', slug: 'docs/install' },
						{ label: 'Getting started', slug: 'docs/getting-started' },
						{ label: 'Wire into your agent', slug: 'docs/agents' },
						{ label: 'Integrations', slug: 'docs/integrations' },
						{ label: 'Self-hosting and Cloud', slug: 'docs/self-hosting' },
					],
				},
				{
					label: 'Core memory',
					items: [
						{ label: 'Observations', slug: 'docs/observations' },
						{ label: 'Search and context', slug: 'docs/search-context' },
						{ label: 'Entity graph', slug: 'docs/graph' },
						{ label: 'Anchors and verify', slug: 'docs/anchors-verify' },
						{ label: 'Decide', slug: 'docs/decide' },
						{ label: 'Mem archives', slug: 'docs/mem' },
						{ label: 'Portability', slug: 'docs/export-import' },
					],
				},
				{
					label: 'Interfaces',
					items: [
						{ label: 'Web UI', slug: 'docs/ui' },
						{ label: 'MCP tools', slug: 'docs/integrations' },
					],
				},
				{
					label: 'Enterprise',
					items: [
						{ label: 'Self-hosting runbooks', slug: 'docs/enterprise' },
					],
				},
				{
					label: 'Reference',
					items: [
						{ label: 'Config', slug: 'docs/config' },
						{ label: 'LoCoMo eval', slug: 'docs/locomo' },
						{ label: 'Troubleshooting', slug: 'docs/troubleshooting' },
						{ label: 'Command cheat sheet', slug: 'docs/commands' },
					],
				},
			],
			head: [
				{
					tag: 'link',
					attrs: {
						rel: 'alternate',
						type: 'text/plain',
						title: 'llms.txt',
						href: `${site}/llms.txt`,
					},
				},
				{
					tag: 'link',
					attrs: {
						rel: 'alternate',
						type: 'text/plain',
						title: 'llms-full.txt',
						href: `${site}/llms-full.txt`,
					},
				},
				{
					tag: 'link',
					attrs: {
						rel: 'api-catalog',
						type: 'application/linkset+json',
						href: `${site}/.well-known/api-catalog`,
					},
				},
				{
					tag: 'link',
					attrs: {
						rel: 'describedby',
						type: 'text/markdown',
						href: `${site}/auth.md`,
					},
				},
				{
					tag: 'script',
					attrs: {
						src: '/webmcp.js',
						defer: true,
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'robots',
						content: 'index,follow,max-image-preview:large',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:type',
						content: 'website',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:site_name',
						content: 'statefulmemory',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image',
						content: `${site}/og.jpg`,
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:alt',
						content: 'statefulmemory: persistent memory for AI coding agents',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:width',
						content: '1200',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:height',
						content: '675',
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'twitter:card',
						content: 'summary_large_image',
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'twitter:image',
						content: `${site}/og.jpg`,
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'twitter:title',
						content: 'statefulmemory',
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'twitter:description',
						content: description,
					},
				},
				{
					tag: 'script',
					attrs: {
						type: 'application/ld+json',
					},
					content: JSON.stringify({
						'@context': 'https://schema.org',
						'@graph': [
							{
								'@type': 'WebSite',
								name: 'statefulmemory',
								url: site,
								description,
								inLanguage: 'en',
								sameAs: [
									'https://github.com/sanudatta11/statefulmemory',
								],
							},
							{
								'@type': 'SoftwareApplication',
								name: 'statefulmemory',
								description,
								url: site,
								applicationCategory: 'DeveloperApplication',
								operatingSystem: 'Linux, macOS',
								offers: {
									'@type': 'Offer',
									price: '0',
									priceCurrency: 'USD',
								},
								sameAs: [
									'https://github.com/sanudatta11/statefulmemory',
								],
							},
							{
								'@type': 'SoftwareSourceCode',
								name: 'statefulmemory',
								description,
								url: site,
								codeRepository: 'https://github.com/sanudatta11/statefulmemory',
								programmingLanguage: 'Rust',
								license: [
									'https://opensource.org/licenses/MIT',
									'https://www.apache.org/licenses/LICENSE-2.0',
								],
								applicationCategory: 'DeveloperApplication',
								operatingSystem: 'Linux, macOS',
							},
							{
								'@type': 'FAQPage',
								mainEntity: [
									{
										'@type': 'Question',
										name: 'What is statefulmemory?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'statefulmemory is persistent memory for AI coding agents: CLI + MCP, gRPC daemon, and per-project SQLite. Self-host on your machine or team, or use StatefulMemory Cloud managed SaaS.',
										},
									},
									{
										'@type': 'Question',
										name: 'Which AI coding agents work with statefulmemory?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode, VS Code / Copilot, Codex, Gemini CLI, Amazon Q, and any agent that can speak MCP or shell out to the statefulmemory CLI.',
										},
									},
									{
										'@type': 'Question',
										name: 'Does statefulmemory work with local or open-source LLMs?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'Yes. Hybrid BM25 + dense search runs locally. Optional LLM steps (extract, conflict judge, Decide) use whichever agent CLI is on PATH. Pin with STATEFULMEMORY_LLM_BIN, STATEFULMEMORY_LLM_PROVIDER, and STATEFULMEMORY_LLM_MODEL (for example OpenCode + qwen).',
										},
									},
								],
							},
						],
					}),
				},
			],
		}),
		sitemap({
			filter: (page) => !page.includes('/404'),
			changefreq: 'weekly',
			priority: 0.7,
			lastmod: new Date(),
			serialize(item) {
				// Home page gets highest priority; docs share the default.
				const url = item.url.replace(/\/$/, '') || item.url;
				if (url === site || url === `${site}/`) {
					return { ...item, priority: 1.0, changefreq: 'weekly' };
				}
				return item;
			},
		}),
	],
});
