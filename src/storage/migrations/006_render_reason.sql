-- Persist *how* a page's content was obtained: the headless render reason, if
-- any. NULL means the content came from a plain HTTP fetch (no headless render).
-- Non-null values: 'on' (explicit headless.mode=on), 'spa' (Auto-mode SPA
-- heuristic fired), 'bot_challenge' (Auto-mode bot-protection challenge bypass).
--
-- This is content provenance, decided at fetch/populate time alongside the rest
-- of the row, so it is stored with the row and reported on cache hits too. It is
-- surfaced in the `fetch` frontmatter as `headless_render`.
ALTER TABLE pages ADD COLUMN render_reason TEXT;
