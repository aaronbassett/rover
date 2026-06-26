---
id: images
title: Images & captioning
---

# Images & captioning

The `fetch` tool's `images` argument controls what happens to every `<img>` on the page. You can keep the tag, strip it to alt text, download the file, drop it, or caption it with a vision model. The default is `alt_text_only`: no downloads, no model calls.

## Image modes

`images.mode` sets per-image handling. Each of the five modes is a distinct operation.

| `mode` | What happens to each image |
| --- | --- |
| `keep` | Preserves the image tag as `![alt](src)`, still pointing at the remote URL. |
| `alt_text_only` | Replaces each image with its alt text. No tag, no link. The default. |
| `download` | Fetches each image, writes it to the output directory, and rewrites the Markdown to reference the local file. |
| `drop` | Removes every image tag. Nothing replaces it. |
| `caption` | Replaces each image with a model-generated caption. Requires a configured captioner. |

`alt_text_only` is the default because alt text is the part of an image a model can act on, and it costs nothing to keep. Most page images are logos, spacers, and decorative borders with no alt text worth keeping, so this mode drops them and keeps the few that describe something.

Set the mode inline on a `fetch` call:

```jsonc
{
  "url": "https://example.com/article",
  "images": { "mode": "caption", "captioner": "openai" }
}
```

`caption` mode needs at least one configured captioner. The captioner comes from `image_captions.default`, and `images.captioner` overrides it for a single call. The full `fetch` schema lives in [MCP tools](/docs/mcp-tools).

## Captioning

Captioning is always compiled in. There's no Cargo feature flag to enable it, and a default install is missing only one thing: a captioner pointed at a model. Captioning runs through cloud vision models via the `genai` crate, the same client the summarisation backends use.

Declare a captioner in a `[captioners.<name>]` block. The shape mirrors a summariser backend:

```toml
[captioners.openai]
kind = "cloud"
provider = "openai"
model = "gpt-4o-mini"
api_key_env = "OPENAI_API_KEY"

[image_captions]
default = "openai"
```

`provider` accepts `openai`, `anthropic`, `gemini`, `openai_compat`, and the rest of the cloud provider set. `api_key_env` names the environment variable holding the key. Rover reads the value at request time, so the key never lands in the config file. The `image_captions.default` line picks which captioner runs when a call doesn't name one. For the shared backend mechanics, see [Configuration](/docs/configuration) and [Summarisation backends](/docs/backends).

## Local captioning

There's no native local vision backend. To caption locally, point `provider = "openai_compat"` at a vision server you run yourself. Ollama and LM Studio both expose an OpenAI-compatible endpoint, and a vision model like `llama3.2-vision` answers image prompts over it. No API key, no image data leaving the machine.

```toml
[captioners.local]
kind = "cloud"
provider = "openai_compat"
model = "llama3.2-vision"
base_url = "http://localhost:11434"

[image_captions]
default = "local"
```

`base_url` is required for `openai_compat` and gets normalised to end in `/v1/`. Supply `http://localhost:11434` and Rover turns it into `http://localhost:11434/v1/`, so you give the host and it fills in the rest. Leave `api_key_env` off entirely for a keyless local server.

## Which images get captioned

Captioning every image on a page is slow and expensive, so `[image_captions]` gates which images are worth a model call. The defaults screen out icons, spacers, and tracking pixels before any caption request goes out.

| Key | Default | What it gates |
| --- | --- | --- |
| `default` | _(none)_ | The captioner name used when a call doesn't override it. |
| `max_tokens` | `128` | Maximum length of each generated caption. |
| `max_per_page` | `10` | Successful captions per page (cache hits count). The loop stops when this is reached. |
| `min_width` | `200` | Skip anything narrower, in pixels. |
| `min_height` | `200` | Skip anything shorter, in pixels. |
| `max_bytes` | `10 MiB` | Skip anything larger. Downloads stream and abort at this limit. |

The dimension gate is cheap by design. Rover reads width and height from the image file header instead of decoding the whole image, so a 5 MB hero image that fails the size check costs almost nothing to reject. The `min_width` and `min_height` defaults of 200 px screen out the icon-and-spacer layer with no manual allowlist.

`max_per_page` caps spend on image-heavy pages. `max_concurrent` (per-captioner concurrency) and `max_attempts` (total provider call cap) live under `[captioners.<name>]`, not `[image_captions]`. Tune the threshold keys in `[image_captions]`; see [Configuration](/docs/configuration) for the full reference.

## Caption budgets and request handling

The selection loop probes images lazily in document order, stops once `max_per_page` successful captions are reached, and backfills from remaining candidates when a caption fails — bounded by `max_attempts` provider calls and `max_concurrent` concurrency.

Two budgets govern the loop:

| Budget | What it counts | Configured under |
| --- | --- | --- |
| `max_per_page` | Successful captions, including cache hits. | `[image_captions]` |
| `max_attempts` | Provider caption calls only; cache hits and pre-caption skips (dimension/size/budget) are free. | `[captioners.<name>]` |

`max_attempts` unset ⇒ `3 × max_per_page`. `max_concurrent` (also per-captioner) bounds simultaneous provider calls.

Image HTTP is rate-limited per host via `[rate_limit].per_domain_concurrency`. HTTP 429 triggers a bounded retry honoring `Retry-After`, clamped to `[rate_limit].retry_after_ceiling`. Downloads stream and abort at `max_bytes`, so memory use is bounded regardless of image size.

## Caption cache

Caption results are cached by image content hash and reused on repeated fetches. The cache saves the VLM call; the image must be downloaded on a hit to derive the key.

Configure under `[image_captions.cache]`:

| Key | Default | Effect |
| --- | --- | --- |
| `enabled` | `true` | `false` skips both cache lookup and insert. |
| `ttl` | `[cache].max_ttl` | Caption row TTL. Unset inherits `[cache].max_ttl`. |
| `restrict_to` | `none` | Key scope (see below). |
| `store_raw_image` | `false` | `true` stores the zstd-compressed image bytes alongside the caption. |

`restrict_to` controls which prior captions count as a hit:

| Value | Scope |
| --- | --- |
| `none` | Image bytes only — the same bytes reuse the caption anywhere. |
| `host` | Same image bytes and image hostname. |
| `page` | Same image bytes and image `src` URL. The key is over the image's own URL, not the containing page. |

Changing `restrict_to` re-derives keys and silently invalidates prior rows.

## Reading the results

Every image the pipeline touches reports its own outcome. The `fetch` response carries an `images_processed` list, one entry per image, and the same data renders into the document's frontmatter. Each entry names the `src`, a `decision` of `captioned` or `skipped`, and a `reason` when the image was skipped:

| `reason` | Why the image was skipped |
| --- | --- |
| `below_min_dimensions` | Smaller than `min_width` × `min_height`. |
| `above_max_bytes` | Larger than `max_bytes`. |
| `per_page_budget` | Past the `max_per_page` cap for this page. |
| `captioner_error` | The captioner was attempted and failed; the entry carries the error string. |

Each entry also carries the detail behind its decision: the measured dimensions, the byte count, the caption text on a hit, or the error string on a failure. The frontmatter adds three running counters, `images_seen`, `images_downloaded`, and `images_failed`. A skipped image isn't a failed one. `per_page_budget` and `below_min_dimensions` are the gates doing their job, not errors. For where these fields sit in the frontmatter envelope, see [Anatomy of a Rover document](/docs/output).

## Security

Every image download is validated against the active SSRF policy, exactly like the page fetch itself, and so is every dimension or byte probe the caption gate runs. That check rejects a literal-IP target such as a cloud-metadata endpoint, whether the URL came from the page body or an `<img>` Rover was about to caption. A page that embeds `<img src="http://169.254.169.254/latest/meta-data/">` gets the same rejection the page URL would. For the SSRF levels and what each one blocks, see [Security & threat model](/docs/security/).

Captioning does nothing until you configure a captioner. The rest of the passes that work the same way are covered in [Optional features](/docs/features).
