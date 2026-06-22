import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  docs: [
    { type: 'category', label: 'Getting started', collapsed: false, items: ['intro'] },
    { type: 'category', label: 'Reference', collapsed: false,
      items: ['cli','mcp-tools','configuration','backends','features','security','versioning','releasing'] },
  ],
};

export default sidebars;
