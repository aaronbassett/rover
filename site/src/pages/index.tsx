import type {ReactNode} from 'react';
import Layout from '@theme/Layout';
import Link from '@docusaurus/Link';
import ThemedImage from '@theme/ThemedImage';
import useBaseUrl from '@docusaurus/useBaseUrl';
import styles from './index.module.css';
import HomeSections from '@site/src/components/home/HomeSections';

export default function Home(): ReactNode {
  return (
    <Layout title="Rover" description="Turn the web into clean, token-efficient Markdown your agent can trust.">
      <header className={styles.hero}>
        <div className={styles.heroGrid}>
          <div>
            <p className="rover-kicker">An MCP server for the open web</p>
            <h1 className={styles.heroTitle}>
              Give your agent the web it can actually <em>trust</em>
            </h1>
            <p className={`rover-dek ${styles.heroDek}`}>
              Rover fetches a URL, strips the ads and chrome, and hands your agent clean Markdown
              with a token count attached. Every page comes back wrapped as untrusted data, not
              instructions.
            </p>
            <div className={styles.heroActions}>
              <code className={styles.cmd}>claude mcp add rover -- rover mcp</code>
              <Link className="button button--primary button--lg" to="/docs/install">Install in one command</Link>
              <Link className="button button--secondary button--lg" to="/docs/intro">Read the docs →</Link>
              <Link className="button button--secondary button--lg" href="https://github.com/aaronbassett/rover">GitHub</Link>
            </div>
            <p className={styles.badges}>
              <span>Clean Markdown</span><span>Token budgeting</span><span>Prompt-injection guard</span><span>Single-user local</span>
            </p>
          </div>
          <figure className={styles.heroFigure}>
            <ThemedImage
              alt="The Rover, on point — an estate-seal engraving"
              sources={{ light: useBaseUrl('/img/hero-seal-light.png'), dark: useBaseUrl('/img/hero-seal-dark.png') }}
            />
            <figcaption className="rover-dek">Pl. I — The Rover, on point.</figcaption>
          </figure>
        </div>
      </header>
      <main><HomeSections /></main>
    </Layout>
  );
}
