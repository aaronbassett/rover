import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: [
    {
      type: 'category',
      label: 'Getting started',
      collapsed: false,
      items: ['intro', 'install', 'quickstart'],
    },
    {
      type: 'category',
      label: 'Guides',
      collapsed: false,
      items: [
        'output',
        'trust',
        'token-budgets',
        'summarizing',
        'caching',
        'dynamic-pages',
        'images',
        'batch',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      collapsed: false,
      items: [
        'mcp-tools',
        'cli',
        'configuration',
        'backends',
        'features',
        'security',
        'versioning',
      ],
    },
    {
      type: 'category',
      label: 'Project',
      collapsed: true,
      items: ['releasing'],
    },
  ],
};

export default sidebars;
