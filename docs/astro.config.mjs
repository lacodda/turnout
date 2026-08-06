// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://lacodda.github.io',
	base: '/turnout',
	integrations: [
		starlight({
			title: 'turnout',
			description: "A developer's switchyard: point local apps at any backend stand, keep servers and secrets at hand, build and deploy from any directory.",
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/lacodda/turnout' }],
			editLink: {
				baseUrl: 'https://github.com/lacodda/turnout/edit/main/docs/',
			},
			sidebar: [
				{ label: 'Getting Started', slug: 'getting-started' },
				{
					label: 'Concepts',
					items: [{ autogenerate: { directory: 'concepts' } }],
				},
				{
					label: 'Reference',
					items: [{ autogenerate: { directory: 'reference' } }],
				},
			],
		}),
	],
});
