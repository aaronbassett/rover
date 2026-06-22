import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

const config: Config = {
  title: 'Rover',
  tagline: 'Turn the web into clean, token-efficient Markdown your agent can trust.',
  favicon: 'img/favicon.png',

  // Set the production url of your site here
  url: 'https://rover-fetch.com',
  // Set the /<baseUrl>/ pathname under which your site is served
  baseUrl: '/',
  trailingSlash: false,

  organizationName: 'aaronbassett',
  projectName: 'rover',

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'throw',

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  plugins: [
    [
      '@signalwire/docusaurus-plugin-llms-txt',
      {
        // Per-route clean Markdown twins (append .md to any URL) + llms.txt index.
        // Disable the whole-site concatenation (llms-full.txt) per spec.
        content: {
          enableMarkdownFiles: true,   // emit <route>.md beside every page
          enableLlmsFullTxt: false,    // keep llms-full.txt OFF per spec
          includeDocs: true,
          includeBlog: false,
          includePages: false,
        },
      },
    ],
  ],

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          editUrl: 'https://github.com/aaronbassett/rover/tree/main/site/',
        },
        blog: false, // Blog disabled — out of scope for rover-fetch.com docs site
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    // Replace with your project's social card
    image: 'img/docusaurus-social-card.jpg',
    colorMode: { defaultMode: 'light', respectPrefersColorScheme: true },
    navbar: {
      title: 'Rover',
      logo: {
        alt: 'Rover',
        src: 'img/logo.png',
        srcDark: 'img/logo-dark.png',
      },
      items: [
        { to: '/docs/intro', label: 'Docs', position: 'left' },
        { to: '/docs/mcp-tools', label: 'Tools', position: 'left' },
        { to: '/docs/security', label: 'Security', position: 'left' },
        { href: 'https://github.com/aaronbassett/rover', label: 'GitHub', position: 'right' },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            { label: 'Getting started', to: '/docs/intro' },
            { label: 'CLI', to: '/docs/cli' },
            { label: 'MCP tools', to: '/docs/mcp-tools' },
          ],
        },
        {
          title: 'Project',
          items: [
            { label: 'GitHub', href: 'https://github.com/aaronbassett/rover' },
            { label: 'crates.io', href: 'https://crates.io/crates/rover-fetch' },
            { label: 'llms.txt', to: 'pathname:///llms.txt' },
          ],
        },
      ],
      copyright: 'MIT / Apache-2.0 · Rover',
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
