# Remote inpaint server

Runs a strong inpainting model (FLUX.1 Fill, 12B) on a rented GPU and serves
it over HTTP to koharu's **Remote Inpaint (HTTP)** engine. The laptop's 6GB
card can't fit this model; a rented RTX 4090 (24GB) can.

**You only pay while a pod is running. Stop the pod when you finish a
session.** A 4090 is roughly $0.35–0.70/hour depending on availability tier.

## One-time preparation

1. **RunPod account** — sign up at <https://runpod.io>, add ~$10 of credit
   (Billing → Add funds). No subscription; credit only drains while a pod runs.
2. **Hugging Face access** — the FLUX.1-Fill-dev weights are license-gated:
   - create an account at <https://huggingface.co>
   - open <https://huggingface.co/black-forest-labs/FLUX.1-Fill-dev> and click
     *Agree and access repository*
   - create a **read** token at <https://huggingface.co/settings/tokens> and
     keep it handy (this is `HF_TOKEN` below)
3. **Invent an API key** — any random string (e.g. from a password generator).
   It protects your server; you'll give the same string to the pod and to
   koharu.

## Starting a pod (each session)

1. RunPod console → **Pods → Deploy**. Pick a **RTX 4090** (24GB),
   *On-Demand*. Community Cloud is cheaper; Secure Cloud is more reliable.
2. Template: the default **RunPod PyTorch** template is fine. Before
   deploying, click *Edit Template*:
   - **Container Disk / Volume**: give it at least **60 GB** (the model is
     ~30 GB and pip installs are chunky).
   - **Expose HTTP Ports**: add `8000`.
   - **Environment variables**: add
     | name | value |
     |---|---|
     | `API_KEY` | the key you invented |
     | `HF_TOKEN` | your Hugging Face read token |
3. Deploy, wait until the pod is *Running*, then open **Connect →
   Jupyter Lab** (port 8888).
4. In Jupyter Lab, drag-and-drop these three files from this folder into the
   file browser: `server.py`, `requirements.txt`, `setup.sh`.
5. Open a Terminal (File → New → Terminal) and run:

   ```bash
   bash setup.sh
   ```

   First start downloads ~30 GB of weights — expect 5–15 minutes. The server
   is ready when the log says `backend ready: flux.1-fill-dev`.
6. Your endpoint URL is shown under **Connect → HTTP Service [Port 8000]**,
   shaped like `https://<pod-id>-8000.proxy.runpod.net`. Check it:

   ```bash
   curl -H "Authorization: Bearer <API_KEY>" https://<pod-id>-8000.proxy.runpod.net/health
   ```

### Cheap smoke test first (recommended)

Before letting it download 30 GB, verify the wiring for pennies: in step 2
also add env var `MODEL` = `opencv`. The server then starts in seconds and
inpaints instantly (crudely) on CPU. Once koharu talks to it successfully,
remove the variable and restart `setup.sh` to get the real model.

## Pointing koharu at it

Edit `%LOCALAPPDATA%\Koharu\remote_inpaint.toml` on the laptop (the file is
auto-created with comments the first time the engine runs):

```toml
endpoint = "https://<pod-id>-8000.proxy.runpod.net"
api_key = "<the key you invented>"
mask = "bubble"
```

Then select **Remote Inpaint (HTTP)** as the inpainter (Settings → pipeline,
or `PATCH /api/v1/config {"pipeline":{"inpainter":"remote-inpaint"}}`) and run
Inpaint as usual. Switch back to **Lama Manga** any time — the remote engine
is opt-in and nothing else changes.

## Stopping (important — this is the money part)

RunPod console → your pod → **Stop**. A stopped pod bills only a small disk
fee (cents/day) and keeps the downloaded weights, so the next **Start** skips
the 30 GB download. **Terminate** deletes everything including the weights —
use it when you won't come back for weeks.

## Protocol (for reference / curl)

```
GET  /health                          -> {"status":"ok","model":...,"ready":true}
POST /inpaint  multipart/form-data:
     image     PNG/WebP/JPEG, the full page
     mask      PNG, white pixels = repaint
     prompt    optional text (diffusion backends)
     steps     optional int   (default 28)
     guidance  optional float (default 30)
     -> image/png, full page, same dimensions
Authorization: Bearer <API_KEY> on every request when API_KEY is set.
```

```bash
curl -H "Authorization: Bearer $KEY" \
     -F image=@page.png -F mask=@mask.png \
     https://<pod-id>-8000.proxy.runpod.net/inpaint -o result.png
```

## Local mock (no GPU, no money)

`bun run mock_server.ts` starts an echo server on `http://127.0.0.1:8787`
that returns the mask as the "result" — good enough to verify the whole
koharu client path end-to-end.

## If FLUX.1 Fill disappoints on manga

Plan B backends, in order: SDXL inpainting (smaller/faster, fits the same
pod), or plain LaMa on the 4090 purely for speed (`mask = "glyph"` in
`remote_inpaint.toml` then — LaMa-style erasers want glyph outlines, not
whole bubbles).
