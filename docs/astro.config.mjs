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
			// The 404 page is a content page (src/content/docs/404.md): Starlight's own
			// route looks for that entry and warns on every build when it is missing.
			disable404Route: true,
			description: "A developer's switchyard: point local apps at any backend stand, keep servers and secrets at hand, build and deploy from any directory.",
			logo: {
				src: './src/assets/logo.svg',
				alt: 'turnout',
			},
			favicon: '/favicon.svg',
			customCss: ['./src/styles/brand.css'],
			head: [
				{ tag: 'link', attrs: { rel: 'apple-touch-icon', href: '/turnout/apple-touch-icon.png' } },
				{ tag: 'meta', attrs: { property: 'og:image', content: 'https://raw.githubusercontent.com/lacodda/turnout/main/assets/social-preview.png' } },
				{ tag: 'meta', attrs: { name: 'twitter:card', content: 'summary_large_image' } },
			],
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/lacodda/turnout' }],
			editLink: {
				baseUrl: 'https://github.com/lacodda/turnout/edit/main/docs/',
			},
			sidebar: [
				{ label: 'Getting Started', slug: 'getting-started' },
				{
					label: 'Guides',
					items: [{ autogenerate: { directory: 'guides' } }],
				},
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
