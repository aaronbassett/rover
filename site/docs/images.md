---
id: images
title: Images & captioning
---

# Images & captioning

**A page's images are usually noise, sometimes signal, and you decide which.** Rover's `fetch` tool takes an `images` argument that controls how every `<img>` on the page is handled — keep it, strip it to alt text, download it, drop it, or hand it to a vision model for a caption. The default leans toward cheap: alt text only, no downloads, no model calls. The other modes are there for when an image carries something the surrounding text doesn't.

## Image modes

`images.mode` sets per-image handling, and the five modes are genuinely different operations — not five ways to format the same tag.

| `mode` | What happens to each image |
| --- | --- |
| `keep` | Preserves the image tag as `![alt](src)`. The Markdown still points at the remote URL. |
| `alt_text_only` | Replaces each image with just its alt text — no tag, no link. This is the default. |
| `download` | Fetches each image, writes it to the output directory, and rewrites the Markdown to reference the local file. |
| `drop` | Removes every image tag. Nothing replaces it. |
| `caption` | Replaces each image with a model-generated caption. Requires a configured captioner. |

`alt_text_only` is the default because alt text is the part of an image a model can usually act on, and it costs nothing. Most page images — logos, spacers, decorative borders — have no alt text worth keeping anyway, so this mode quietly drops them while preserving the few that describe something.

Set the mode inline on a `fetch` call:

```jsonc
{
  "url": "https://example.com/article",
  "images": { "mode": "caption", "captioner": "openai" }
}
```

`caption` mode needs at least one configured captioner. The captioner used comes from `[image_captions] default`, and `images.captioner` overrides it for a single call. See [MCP tools](/docs/mcp-tools) for the full `fetch` schema.

## Captioning

**Captioning is always compiled in. There is no Cargo feature flag to enable it.** Every Rover binary can caption images out of the box — the only thing missing from a default install is a captioner you've pointed at a model. Captioning runs through cloud vision models via the `genai` crate, the same client the summarisation backends use.

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

`provider` accepts `openai`, `anthropic`, `gemini`, `openai_compat`, and the rest of the cloud provider set. `api_key_env` names the environment variable holding the key — Rover reads the value at request time, so the key never lands in the config file. The `[image_captions] default` line picks which captioner runs when a call doesn't name one. See [Configuration](/docs/configuration) and [Summarisation backends](/docs/backends) for the shared backend mechanics.

## Local captioning

There is no native local vision backend — for local captioning, point `provider = "openai_compat"` at a vision server you run yourself. Ollama and LM Studio both expose an OpenAI-compatible endpoint, and a vision model like `llama3.2-vision` answers image prompts over it. No API key required, no image data leaving the machine.

```toml
[captioners.local]
kind = "cloud"
provider = "openai_compat"
model = "llama3.2-vision"
base_url = "http://localhost:11434"

[image_captions]
default = "local"
```

`base_url` is required for `openai_compat` and gets normalised to end in `/v1/` — `http://localhost:11434` becomes `http://localhost:11434/v1/`, so you can supply the host without the path and Rover fills in the rest. Leave `api_key_env` off entirely for a keyless local server.

## Which images get captioned

Captioning every image on a page is slow and expensive, so `[image_captions]` gates which images are worth the model call. The defaults screen out the images that almost never carry meaning — icons, spacers, tracking pixels — before any caption request is made.

| Key | Default | What it gates |
| --- | --- | --- |
| `default` | _(none)_ | The captioner name used when a call doesn't override it. |
| `max_tokens` | `50` | Maximum length of each generated caption. |
| `max_per_page` | `10` | Caption the first N qualifying images; drop the rest. |
| `min_width` | `200` | Skip anything narrower, in pixels. |
| `min_height` | `200` | Skip anything shorter, in pixels. |
| `max_bytes` | `10 MiB` | Skip anything larger. |
| `max_concurrent` | `2` | How many captions run in parallel. |

The dimension gate is cheap by design. Rover reads width and height from the image file header, not by decoding the whole image — so a 5 MB hero image that fails the size check costs almost nothing to reject. The `min_width` / `min_height` defaults of 200 px exist to screen out the icon-and-spacer layer of a page without a manual allowlist.

`max_per_page` caps spend on image-heavy pages: the first ten qualifying images get captioned, and everything after that is dropped rather than queued. Tune these in `[image_captions]` or per call via the `images` argument. See [Configuration](/docs/configuration) for the full file layout.

## Reading the results

**Every image the pipeline touches reports its own outcome.** The `fetch` response carries an `images_processed` list — one entry per image — and the same data renders into the document's frontmatter. Each entry names the `src`, a `decision` of `captioned` or `skipped`, and a `reason` when the image was skipped:

| `reason` | Why the image was skipped |
| --- | --- |
| `below_min_dimensions` | Smaller than `min_width` × `min_height`. |
| `above_max_bytes` | Larger than `max_bytes`. |
| `per_page_budget` | Past the `max_per_page` cap for this page. |
| `captioner_error` | The captioner was attempted and failed; the entry carries the error string. |

Each entry also carries the supporting detail behind its decision — the measured dimensions, the byte count, the caption text on a hit, or the error string on a captioner failure. The frontmatter adds three running counters across the page: `images_seen`, `images_downloaded`, and `images_failed`. A skipped image is not a failed one — `per_page_budget` and `below_min_dimensions` are the gates doing their job, not errors. See [Anatomy of a Rover document](/docs/output) for where these fields sit in the frontmatter envelope.

## Security

Image fetches are not a side door around the fetch policy. Every image download — and every dimension or byte probe the caption gate runs — is validated against the active SSRF policy exactly like the page fetch itself. That is the check that rejects a literal-IP target such as a cloud-metadata endpoint, whether the URL came from the page body or from an `<img>` Rover was about to caption. A page that embeds `<img src="http://169.254.169.254/latest/meta-data/">` gets the same rejection the page URL would. See [Security & threat model](/docs/security/) for the SSRF levels and what each one blocks.

Captioning is one of several passes you can leave off entirely. It is an optional feature in the sense that it does nothing until you configure a captioner — see [Optional features](/docs/features) for the rest of the passes that work the same way.
