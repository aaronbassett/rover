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
              Turn the web into Markdown your agent can <em>trust</em>.
            </h1>
            <p className={`rover-dek ${styles.heroDek}`}>
              Rover fetches a URL, strips the chrome, extracts the real content, counts the
              tokens, and wraps it so the model knows it&apos;s untrusted data — not instructions.
            </p>
            <div className={styles.heroActions}>
              <code className={styles.cmd}>claude mcp add rover -- rover mcp</code>
              <Link className="button button--primary button--lg" to="/docs/intro">Read the docs →</Link>
              <Link className="button button--secondary button--lg" href="https://github.com/aaronbassett/rover">GitHub</Link>
            </div>
            <p className={styles.badges}>
              <span>SSRF protection</span><span>Injection guard</span><span>HTTP caching</span><span>Token budgets</span>
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
