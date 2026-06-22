import type {ReactNode} from 'react';
import Link from '@docusaurus/Link';
import styles from './HomeSections.module.css';

const WALLS = [
  ['Boilerplate & chrome', 'Ads, nav, and cookie banners drown the content — Rover strips them to clean Markdown.'],
  ['JavaScript pages', 'SPAs return an empty root div. Optional headless rendering gets the real content.'],
  ['Wasted tokens', 'HTTP-aware caching, token budgets, and summarise-to-fit keep costs down.'],
  ['Untrusted content', 'A page can smuggle "ignore your instructions". Rover wraps every response as data, not instructions.'],
];
const TOOLS = [
  ['fetch', 'Single URL → cleaned Markdown. Caching, headless, image modes, token budgeting, inline summarisation.'],
  ['batch_fetch', 'Fetch N URLs concurrently with per-domain rate limiting; streams NDJSON progress.'],
  ['summarize', 'Compact a page via extractive (offline) or cloud backends. Steerable with focus/preserve/target_tokens.'],
  ['get_metadata', 'Schema.org, Open Graph, and Twitter Card metadata without the full body.'],
  ['count_tokens', "Estimate a URL’s token cost across cl100k / o200k / claude / llama3 / qwen3."],
];

export default function HomeSections(): ReactNode {
  return (
    <div className={styles.wrap}>
      <section className={styles.section}>
        <p className="rover-kicker">Why Rover</p>
        <h2 className={styles.h2}>Agents browsing the live web hit four walls.</h2>
        <div className={styles.grid4}>
          {WALLS.map(([t, d]) => (
            <div key={t} className={styles.card}><h3>{t}</h3><p>{d}</p></div>
          ))}
        </div>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">How your agent gets the web</p>
        <h2 className={styles.h2}>Rover vs WebFetch vs wget</h2>
        <table className={styles.compare}>
          <thead><tr><th></th><th>Rover</th><th>WebFetch</th><th>wget</th></tr></thead>
          <tbody>
            <tr><td>Returns clean Markdown doc</td><td>✅</td><td>◻️ lossy answer</td><td>❌ raw HTML</td></tr>
            <tr><td>Reusable across calls</td><td>✅ cached, stable hash</td><td>❌ re-runs model</td><td>✅ raw file</td></tr>
            <tr><td>Token budgeting & counts</td><td>✅</td><td>❌</td><td>❌</td></tr>
            <tr><td>SSRF / private-network guard</td><td>✅ 5 levels</td><td>◻️</td><td>❌</td></tr>
            <tr><td>Prompt-injection guard</td><td>✅ layered</td><td>❌</td><td>—</td></tr>
          </tbody>
        </table>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">The MCP tools</p>
        <h2 className={styles.h2}>Five tools your agent gets on day one.</h2>
        <div className={styles.tools}>
          {TOOLS.map(([n, d]) => (
            <div key={n} className={styles.tool}><code>{n}</code><p>{d}</p></div>
          ))}
        </div>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">Security &amp; trust</p>
        <h2 className={styles.h2}>The web is hostile by default. Rover treats it that way.</h2>
        <div className={styles.grid4}>
          <div className={styles.card}><h3>Prompt-injection guard</h3><p>Nonce wrapper + pattern detector + optional ONNX classifier.</p></div>
          <div className={styles.card}><h3>SSRF protection</h3><p>Five levels + dial-time re-check that closes the DNS-rebinding window.</p></div>
          <div className={styles.card}><h3>Secret redaction</h3><p>URL secrets and Authorization credentials scrubbed before logging.</p></div>
          <div className={styles.card}><h3>Short cache TTL</h3><p>15-minute default so poisoned/changed content has a small blast radius.</p></div>
        </div>
      </section>

      <hr className="rover-rule" />

      <section className={`${styles.section} ${styles.cta}`}>
        <h2 className={styles.h2}>Wire it into your agent.</h2>
        <code className={styles.ctaCmd}>claude mcp add rover -- rover mcp</code>
        <Link className="button button--primary button--lg" to="/docs/intro">Read the docs →</Link>
      </section>
    </div>
  );
}
