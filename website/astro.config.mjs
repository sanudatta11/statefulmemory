// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

const site = 'https://statefulmemory.dev';
const description =
	'Persistent memory for coding agents, with an on-device System-1 that decides. Self-host or StatefulMemory Cloud SaaS. CLI + MCP, daemon, per-project SQLite, native MLX on Apple Silicon.';

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
				Head: './src/components/Head.astro',
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
						{ label: 'Laya System-1', slug: 'docs/laya' },
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
					tag: 'link',
					attrs: {
						rel: 'sitemap',
						type: 'application/xml',
						title: 'Sitemap',
						href: `${site}/sitemap.xml`,
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
						content: 'statefulmemory: persistent memory for AI coding agents — coding agents forget, statefulmemory remembers, Laya decides on-device',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:width',
						content: '1920',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:height',
						content: '1080',
					},
				},
				{
					tag: 'meta',
					attrs: {
						property: 'og:image:type',
						content: 'image/jpeg',
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
								'@type': 'Organization',
								name: 'statefulmemory',
								url: site,
								logo: {
									'@type': 'ImageObject',
									url: `${site}/favicon.svg`,
								},
								sameAs: [
									'https://github.com/sanudatta11/statefulmemory',
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
