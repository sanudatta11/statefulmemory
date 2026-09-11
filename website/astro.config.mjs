// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';

// https://astro.build/config
export default defineConfig({
	site: 'https://memlayer.org',
	base: '/',
	integrations: [
		starlight({
			title: 'memlayer',
			description:
				'Persistent memory for AI coding agents — local, per-project, no cloud.',
			favicon: '/favicon.svg',
			customCss: ['./src/styles/custom.css'],
			components: {
				Hero: './src/components/Hero.astro',
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
			sidebar: [
				{
					label: 'Docs',
					items: [
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
						property: 'og:type',
						content: 'website',
					},
				},
				{
					tag: 'meta',
					attrs: {
						name: 'twitter:card',
						content: 'summary',
					},
				},
			],
		}),
		sitemap(),
	],
});
