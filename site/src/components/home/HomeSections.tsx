import type {ReactNode} from 'react';
import Link from '@docusaurus/Link';
import styles from './HomeSections.module.css';

const WALLS: [string, ReactNode][] = [
  ['Boilerplate buries the content', <>Ads, nav, and cookie banners drown the real text, and your token budget pays for every byte of it.</>],
  ['JavaScript hides the page', <>A modern SPA hands a non-browser an empty <code>{'<div id="root">'}</code>, so the model reasons over a shell.</>],
  ['Repeated fetches cost real money', <>Hammering the same URL burns tokens, time, and money, and ignores the politeness rules every host expects.</>],
  ['Fetched content is hostile', <>A page can carry &ldquo;ignore your instructions&rdquo; straight into the context window, and most fetch tools hand it over raw.</>],
];

const TOOLS: [string, ReactNode][] = [
  ['fetch', <>Turn a single URL into cleaned Markdown, with caching, headless rendering, image modes, token budgeting, and inline summarisation.</>],
  ['batch_fetch', <>Fetch N URLs concurrently with per-domain rate limiting, streaming NDJSON progress as each one lands.</>],
  ['summarize', <>Compact a page through an extractive offline backend or a cloud one, steered by <code>focus</code>, <code>preserve</code>, and <code>target_tokens</code>.</>],
  ['get_metadata', <>Pull Schema.org, Open Graph, and Twitter Card metadata without fetching the full body.</>],
  ['count_tokens', <>Estimate a URL&rsquo;s token cost across <code>cl100k</code>, <code>o200k</code>, <code>claude</code>, <code>llama3</code>, and <code>qwen3</code> before you pay for it.</>],
];

const SECURITY: [string, ReactNode][] = [
  ['Prompt-injection guard', <>A per-response nonce wrapper holds by construction, backed by a pattern detector and an optional ONNX classifier for the phrasings rules miss.</>],
  ['SSRF protection', <>Five policy levels plus a dial-time re-check that re-validates every resolved address before the socket opens, closing the DNS-rebinding window.</>],
  ['Secret redaction', <>URL secrets and <code>Authorization</code> credentials are scrubbed before any event reaches a log.</>],
  ['Short cache TTL', <>The 15-minute default keeps the blast radius small when content is poisoned or quietly changes.</>],
];

const FRONTMATTER = `---
url: "https://en.wikipedia.org/wiki/Rust_(programming_language)"
title: "Rust (programming language) - Wikipedia"
content_hash: "sha256:b3e9…"
estimated_tokens: 14823
tokenizer: "o200k"
extraction_quality: 0.98
---`;

export default function HomeSections(): ReactNode {
  return (
    <div className={styles.wrap}>
      <section className={styles.section}>
        <p className="rover-kicker">Why Rover</p>
        <h2 className={styles.h2}>Agents hit the same four walls on the live web.</h2>
        <div className={styles.grid4}>
          {WALLS.map(([t, d]) => (
            <div key={t} className={styles.card}><h3>{t}</h3><p>{d}</p></div>
          ))}
        </div>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">The output</p>
        <h2 className={styles.h2}>A document, not a one-shot answer.</h2>
        <div className={styles.getback}>
          <p className={styles.getbackText}>
            Rover returns a Markdown document with YAML frontmatter: the content hash, the token
            count, the extraction-quality score. Cache it, re-read it, diff it against the next
            fetch — it stays stable instead of dissolving into a fresh model answer every prompt.
          </p>
          <figure className={styles.sampleFigure}>
            <pre className={styles.sample}><code>{FRONTMATTER}</code></pre>
            <figcaption className="rover-dek">Frontmatter travels with the content.</figcaption>
          </figure>
        </div>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">How it compares</p>
        <h2 className={styles.h2}>Rover vs WebFetch vs wget</h2>
        <table className={styles.compare}>
          <thead><tr><th></th><th>Rover</th><th>WebFetch</th><th>wget</th></tr></thead>
          <tbody>
            <tr><td>What you get back</td><td>✅ Clean Markdown document</td><td>◻️ Lossy per-prompt answer</td><td>❌ Raw HTML</td></tr>
            <tr><td>Reusable across calls</td><td>✅ Cached, stable hash</td><td>❌ Re-runs the model</td><td>✅ Raw file</td></tr>
            <tr><td>Token budgeting &amp; counts</td><td>✅</td><td>❌</td><td>❌</td></tr>
            <tr><td>HTTP-aware caching</td><td>✅ TTL / ETag / SWR</td><td>◻️ Flat 15-min</td><td>◻️ Timestamping</td></tr>
            <tr><td>SSRF / private-network guard</td><td>✅ 5 levels + dial-time recheck</td><td>◻️</td><td>❌</td></tr>
            <tr><td>Prompt-injection guard</td><td>✅ Layered</td><td>❌</td><td>—</td></tr>
          </tbody>
        </table>
        <p className={styles.legend}>✅ full · ◻️ partial · ❌ none · — not applicable</p>
      </section>

      <hr className="rover-rule" />

      <section className={styles.section}>
        <p className="rover-kicker">The tools</p>
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
          {SECURITY.map(([t, d]) => (
            <div key={t} className={styles.card}><h3>{t}</h3><p>{d}</p></div>
          ))}
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
