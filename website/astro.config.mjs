// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

const site = 'https://memlayer.org';
const description =
	'Persistent memory for coding agents — self-host or Memlayer Cloud SaaS. CLI + MCP, daemon, per-project SQLite.';

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
			title: 'memlayer',
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
					href: 'https://github.com/sanudatta11/memlayer',
				},
			],
			editLink: {
				baseUrl: 'https://github.com/sanudatta11/memlayer/edit/main/website/',
			},
			lastUpdated: true,
			pagination: true,
			sidebar: [
				{
					label: 'Start here',
					items: [
						{ label: 'Overview', slug: '' },
						{ label: 'Why memlayer', slug: 'docs/why-memlayer' },
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
						{ label: 'Anchors and verify', slug: 'docs/anchors-verify' },
						{ label: 'Decide', slug: 'docs/decide' },
						{ label: 'Mem archives', slug: 'docs/mem' },
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
						content: 'memlayer',
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
						content: 'memlayer: persistent memory for AI coding agents',
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
						content: 'memlayer',
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
								name: 'memlayer',
								url: site,
								description,
								inLanguage: 'en',
								sameAs: [
									'https://github.com/sanudatta11/memlayer',
								],
							},
							{
								'@type': 'SoftwareApplication',
								name: 'memlayer',
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
									'https://github.com/sanudatta11/memlayer',
								],
							},
							{
								'@type': 'SoftwareSourceCode',
								name: 'memlayer',
								description,
								url: site,
								codeRepository: 'https://github.com/sanudatta11/memlayer',
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
										name: 'What is memlayer?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'memlayer is persistent memory for AI coding agents: CLI + MCP, gRPC daemon, and per-project SQLite. Self-host on your machine or team, or use Memlayer Cloud managed SaaS.',
										},
									},
									{
										'@type': 'Question',
										name: 'Which AI coding agents work with memlayer?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'Claude Code, Cursor, Windsurf, Antigravity, OpenCode, Kimi Code, ZCode, VS Code / Copilot, Codex, Gemini CLI, Amazon Q, and any agent that can speak MCP or shell out to the memlayer CLI.',
										},
									},
									{
										'@type': 'Question',
										name: 'Does memlayer work with local or open-source LLMs?',
										acceptedAnswer: {
											'@type': 'Answer',
											text: 'Yes. Hybrid BM25 + dense search runs locally. Optional LLM steps (extract, conflict judge, Decide) use whichever agent CLI is on PATH. Pin with MEMLAYER_LLM_BIN, MEMLAYER_LLM_PROVIDER, and MEMLAYER_LLM_MODEL (for example OpenCode + qwen).',
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
