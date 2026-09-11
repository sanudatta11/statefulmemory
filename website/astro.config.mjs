// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

const site = 'https://memlayer.org';
const description =
	'Persistent memory for AI coding agents — local, per-project, no cloud.';

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
					label: 'Docs',
					items: [
						{ label: 'Overview', slug: '' },
						{ label: 'Getting started', slug: 'docs/getting-started' },
						{ label: 'Install', slug: 'docs/install' },
						{ label: 'Wire into your agent', slug: 'docs/agents' },
						{ label: 'Everyday commands', slug: 'docs/commands' },
						{ label: 'Config', slug: 'docs/config' },
						{ label: 'Troubleshooting', slug: 'docs/troubleshooting' },
					],
				},
			],
			head: [
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
						content: 'memlayer — persistent memory for AI coding agents',
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
							},
							{
								'@type': 'SoftwareSourceCode',
								name: 'memlayer',
								description,
								url: site,
								codeRepository: 'https://github.com/sanudatta11/memlayer',
								programmingLanguage: 'Rust',
								license: 'https://opensource.org/licenses/MIT',
								applicationCategory: 'DeveloperApplication',
								operatingSystem: 'Linux, macOS',
							},
						],
					}),
				},
			],
		}),
		sitemap({
			filter: (page) => !page.includes('/404'),
		}),
	],
});
