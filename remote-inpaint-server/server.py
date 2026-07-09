"""Remote inpaint server for koharu's `remote-inpaint` engine.

Endpoints
    GET  /health   -> {"status": "ok", "model": ..., "device": ..., "ready": true}
    POST /inpaint  -> multipart form-data in, image/png out
                      parts:  image (PNG/WebP/JPEG), mask (PNG, white = repaint)
                      fields: prompt?, steps?, guidance?  (diffusion backends only)

Auth: if the API_KEY env var is set, requests must send
`Authorization: Bearer <API_KEY>`. Unset = no auth (fine for a quick test,
not for a pod that stays up).

Environment
    MODEL          flux-fill (default) | opencv
                   opencv = instant CPU inpaint (cv2.INPAINT_TELEA); use it to
                   smoke-test the wiring before downloading 30GB of weights.
    API_KEY        bearer token the client must present
    HF_TOKEN       Hugging Face token with access to the *gated*
                   black-forest-labs/FLUX.1-Fill-dev repo (accept its license
                   on huggingface.co first)
    QUANTIZE       4bit (default) | none
                   4bit  = NF4-quantize the transformer + T5 (fits 24GB VRAM)
                   none  = bf16 with CPU offload (slower, needs lots of RAM)
    DEFAULT_STEPS      inference steps when the client sends none (28)
    DEFAULT_GUIDANCE   guidance scale when the client sends none (30)
    DEFAULT_PROMPT     prompt when the client sends none
    CROP_MARGIN        context padding around each masked area, px (128)
    CROP_MAX_SIDE      crops larger than this are diffused downscaled (1280)

Run:  uvicorn server:app --host 0.0.0.0 --port 8000
"""

import io
import logging
import os
import secrets
import threading

import numpy as np
from fastapi import FastAPI, Form, Request, Response, UploadFile
from fastapi.responses import JSONResponse
from PIL import Image

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("remote-inpaint")

MODEL = os.environ.get("MODEL", "flux-fill").lower()
API_KEY = os.environ.get("API_KEY", "")
QUANTIZE = os.environ.get("QUANTIZE", "4bit").lower()
DEFAULT_STEPS = int(os.environ.get("DEFAULT_STEPS", "28"))
DEFAULT_GUIDANCE = float(os.environ.get("DEFAULT_GUIDANCE", "30"))
DEFAULT_PROMPT = os.environ.get(
    "DEFAULT_PROMPT",
    "an empty speech bubble with a clean solid background, manga page, no text, no letters",
)
CROP_MARGIN = int(os.environ.get("CROP_MARGIN", "128"))
CROP_MAX_SIDE = int(os.environ.get("CROP_MAX_SIDE", "1280"))

app = FastAPI(title="koharu remote inpaint")

# One GPU, one job at a time. FastAPI runs `def` endpoints in a thread pool,
# so concurrent requests queue up on this lock instead of racing the model.
_gpu_lock = threading.Lock()
_backend = None
_backend_error: str | None = None


# ---------------------------------------------------------------------------
# Backends
# ---------------------------------------------------------------------------


class OpenCVBackend:
    """cv2.inpaint — not pretty, but instant and CPU-only. For wiring tests."""

    name = "opencv-telea"
    device = "cpu"

    def inpaint(self, image: Image.Image, mask: Image.Image, prompt, steps, guidance):
        import cv2

        img = cv2.cvtColor(np.array(image.convert("RGB")), cv2.COLOR_RGB2BGR)
        m = (np.array(mask.convert("L")) > 127).astype(np.uint8) * 255
        out = cv2.inpaint(img, m, 3, cv2.INPAINT_TELEA)
        return Image.fromarray(cv2.cvtColor(out, cv2.COLOR_BGR2RGB))


class FluxFillBackend:
    """FLUX.1-Fill-dev via diffusers.

    Rather than diffusing the whole page (slow: a 1400x2000 page is a lot of
    latent tokens), each connected masked area is cropped with CROP_MARGIN of
    context, inpainted separately, and pasted back only under the mask —
    mirrors IOPaint's crop strategy.
    """

    name = "flux.1-fill-dev"

    def __init__(self):
        import torch
        from diffusers import FluxFillPipeline

        self.torch = torch
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        repo = "black-forest-labs/FLUX.1-Fill-dev"

        if QUANTIZE == "4bit":
            log.info("loading %s with 4-bit NF4 quantization (transformer + T5)", repo)
            from diffusers import PipelineQuantizationConfig

            quant = PipelineQuantizationConfig(
                quant_backend="bitsandbytes_4bit",
                quant_kwargs={
                    "load_in_4bit": True,
                    "bnb_4bit_quant_type": "nf4",
                    "bnb_4bit_compute_dtype": torch.bfloat16,
                },
                components_to_quantize=["transformer", "text_encoder_2"],
            )
            self.pipe = FluxFillPipeline.from_pretrained(
                repo, torch_dtype=torch.bfloat16, quantization_config=quant
            )
            self.pipe.to(self.device)
        else:
            log.info("loading %s in bf16 with CPU offload", repo)
            self.pipe = FluxFillPipeline.from_pretrained(repo, torch_dtype=torch.bfloat16)
            self.pipe.enable_model_cpu_offload()

        self.pipe.set_progress_bar_config(disable=True)
        log.info("FLUX.1 Fill ready on %s", self.device)

    # -- crop plumbing ------------------------------------------------------

    @staticmethod
    def _mask_boxes(mask_bin: np.ndarray, margin: int) -> list[tuple[int, int, int, int]]:
        """Padded bounding boxes of the mask's connected components, with
        overlapping boxes merged. Returns (x0, y0, x1, y1) tuples."""
        import cv2

        h, w = mask_bin.shape
        n, _, stats, _ = cv2.connectedComponentsWithStats(mask_bin, connectivity=8)
        boxes = []
        for i in range(1, n):
            x, y, bw, bh = stats[i, 0], stats[i, 1], stats[i, 2], stats[i, 3]
            boxes.append(
                (max(0, x - margin), max(0, y - margin), min(w, x + bw + margin), min(h, y + bh + margin))
            )

        merged = True
        while merged:
            merged = False
            out: list[tuple[int, int, int, int]] = []
            for box in boxes:
                for j, other in enumerate(out):
                    if box[0] < other[2] and other[0] < box[2] and box[1] < other[3] and other[1] < box[3]:
                        out[j] = (
                            min(box[0], other[0]),
                            min(box[1], other[1]),
                            max(box[2], other[2]),
                            max(box[3], other[3]),
                        )
                        merged = True
                        break
                else:
                    out.append(box)
            boxes = out
        return boxes

    @staticmethod
    def _snap_box(box, w, h, multiple=16):
        """Grow the box so its sides are multiples of `multiple`, shifting
        away from image borders when needed."""
        x0, y0, x1, y1 = box

        def grow(a0, a1, limit):
            size = a1 - a0
            pad = (-size) % multiple
            a0 = max(0, a0 - pad // 2)
            a1 = min(limit, a0 + size + pad)
            a0 = max(0, a1 - (size + pad))
            # If the image itself is smaller than the padded size, clamp to a
            # smaller multiple that fits.
            if (a1 - a0) % multiple:
                a1 = a0 + ((a1 - a0) // multiple) * multiple
            return a0, a1

        x0, x1 = grow(x0, x1, w)
        y0, y1 = grow(y0, y1, h)
        return x0, y0, x1, y1

    # -- inference ----------------------------------------------------------

    def inpaint(self, image: Image.Image, mask: Image.Image, prompt, steps, guidance):
        img = image.convert("RGB")
        mask_l = mask.convert("L")
        mask_np = (np.array(mask_l) > 127).astype(np.uint8)
        out = np.array(img)

        boxes = self._mask_boxes(mask_np, CROP_MARGIN)
        log.info("inpainting %d region(s), steps=%d guidance=%.1f", len(boxes), steps, guidance)
        for box in boxes:
            x0, y0, x1, y1 = self._snap_box(box, img.width, img.height)
            cw, ch = x1 - x0, y1 - y0
            if cw < 16 or ch < 16:
                continue
            crop_img = img.crop((x0, y0, x1, y1))
            crop_mask = mask_l.crop((x0, y0, x1, y1))

            # Very large crops get diffused at reduced resolution and the
            # result upscaled back — bubble fills are flat, this is invisible.
            scale = min(1.0, CROP_MAX_SIDE / max(cw, ch))
            if scale < 1.0:
                dw = max(16, int(cw * scale) // 16 * 16)
                dh = max(16, int(ch * scale) // 16 * 16)
                run_img = crop_img.resize((dw, dh), Image.LANCZOS)
                run_mask = crop_mask.resize((dw, dh), Image.LANCZOS)
            else:
                dw, dh = cw, ch
                run_img, run_mask = crop_img, crop_mask

            result = self.pipe(
                prompt=prompt,
                image=run_img,
                mask_image=run_mask,
                height=dh,
                width=dw,
                num_inference_steps=steps,
                guidance_scale=guidance,
                max_sequence_length=512,
            ).images[0]

            if (dw, dh) != (cw, ch):
                result = result.resize((cw, ch), Image.LANCZOS)

            region = mask_np[y0:y1, x0:x1, None].astype(bool)
            out[y0:y1, x0:x1] = np.where(region, np.array(result), out[y0:y1, x0:x1])

        return Image.fromarray(out)


def _load_backend():
    global _backend, _backend_error
    try:
        _backend = OpenCVBackend() if MODEL == "opencv" else FluxFillBackend()
        log.info("backend ready: %s", _backend.name)
    except Exception as exc:  # surfaced via /health and /inpaint
        _backend_error = f"{type(exc).__name__}: {exc}"
        log.exception("backend failed to load")


@app.on_event("startup")
def _startup():
    # Load in a thread so /health responds immediately while weights download.
    threading.Thread(target=_load_backend, daemon=True).start()


# ---------------------------------------------------------------------------
# HTTP layer
# ---------------------------------------------------------------------------


def _auth_error(request: Request) -> Response | None:
    if not API_KEY:
        return None
    supplied = request.headers.get("authorization", "")
    if secrets.compare_digest(supplied, f"Bearer {API_KEY}"):
        return None
    return JSONResponse({"error": "unauthorized"}, status_code=401)


@app.get("/health")
def health(request: Request):
    if err := _auth_error(request):
        return err
    if _backend_error:
        return JSONResponse({"status": "error", "error": _backend_error}, status_code=500)
    if _backend is None:
        return JSONResponse({"status": "loading", "model": MODEL, "ready": False})
    return {"status": "ok", "model": _backend.name, "device": _backend.device, "ready": True}


@app.post("/inpaint")
def inpaint(
    request: Request,
    image: UploadFile,
    mask: UploadFile,
    prompt: str | None = Form(None),
    steps: int | None = Form(None),
    guidance: float | None = Form(None),
):
    if err := _auth_error(request):
        return err
    if _backend_error:
        return JSONResponse({"error": f"backend failed to load: {_backend_error}"}, status_code=500)
    if _backend is None:
        return JSONResponse({"error": "model still loading, retry shortly"}, status_code=503)

    img = Image.open(io.BytesIO(image.file.read()))
    mask_img = Image.open(io.BytesIO(mask.file.read()))
    if img.size != mask_img.size:
        return JSONResponse(
            {"error": f"image {img.size} and mask {mask_img.size} dimensions differ"},
            status_code=400,
        )

    with _gpu_lock:
        result = _backend.inpaint(
            img,
            mask_img,
            prompt or DEFAULT_PROMPT,
            steps or DEFAULT_STEPS,
            guidance if guidance is not None else DEFAULT_GUIDANCE,
        )

    buf = io.BytesIO()
    result.save(buf, format="PNG")
    return Response(content=buf.getvalue(), media_type="image/png")
