mod latents;
mod precomputed;
pub mod qwen;
mod scheduler;
mod transformer;
mod vae;

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use candle_core::{DType, Device, Tensor};
use image::{DynamicImage, GenericImageView, GrayImage, Luma, RgbImage, RgbaImage};
use imageproc::{
    distance_transform::Norm,
    morphology::dilate,
    region_labelling::{Connectivity, connected_components},
};
use koharu_runtime::RuntimeManager;
use tracing::instrument;

use crate::{device, inpainting, loading};

use self::{
    latents::{
        IMAGE_MULTIPLE, expand_mask, image_to_tensor, mask_to_packed_tensor, pack_latents,
        prepare_latent_ids, prepare_mask, prepare_rgb_image, resize_back_if_needed,
        tensor_to_rgb_image, unpack_latents,
    },
    precomputed::Flux2PromptEmbedder,
    scheduler::FlowMatchScheduler,
    transformer::Flux2Transformer,
    vae::Flux2Vae,
};

const FLUX2_REPO: &str = "unsloth/FLUX.2-klein-4B-GGUF";
const FLUX2_GGUF: &str = "flux-2-klein-4b-Q4_K_M.gguf";
const VAE_REPO: &str = "black-forest-labs/FLUX.2-small-decoder";
const VAE_FILE: &str = "diffusion_pytorch_model.safetensors";
const INPAINT_CROP_CONTEXT: u32 = 64;
/// Sample generated/background colour immediately outside the tight erase
/// mask. Sampling inside the mask sees the original lettering and can tint the
/// fill; feathering inside it blends that lettering back into the edge.
const COLOR_MATCH_RING_RADIUS: u8 = 2;
const MAX_COLOR_MATCH_OFFSET: i16 = 64;
const MAX_COLOR_MATCH_SAMPLE_DELTA: i16 = 96;
const MAX_COLOR_MATCH_MEDIAN_DEVIATION: i16 = 24;
const MIN_COLOR_MATCH_SAMPLES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawBounds {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CropBounds {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

koharu_runtime::declare_hf_model_package!(
    id: "model:flux2-klein-4b:transformer-q4-k-m",
    repo: FLUX2_REPO,
    file: FLUX2_GGUF,
    bootstrap: false,
    order: 140,
);
koharu_runtime::declare_hf_model_package!(
    id: "model:flux2-klein-4b:small-decoder",
    repo: VAE_REPO,
    file: VAE_FILE,
    bootstrap: false,
    order: 143,
);

#[derive(Debug, Clone)]
pub struct Flux2KleinPaths {
    pub transformer_gguf: PathBuf,
    pub vae_safetensors: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Flux2InpaintOptions {
    pub num_inference_steps: usize,
    pub strength: f64,
    pub max_pixels: u32,
    pub mask_padding: u8,
}

impl Default for Flux2InpaintOptions {
    fn default() -> Self {
        Self {
            num_inference_steps: 4,
            strength: 1.0,
            max_pixels: 1024 * 1024,
            mask_padding: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Flux2ImageToImageOptions {
    pub num_inference_steps: usize,
    pub strength: f64,
    pub max_pixels: u32,
}

impl Default for Flux2ImageToImageOptions {
    fn default() -> Self {
        Self {
            num_inference_steps: 4,
            strength: 1.0,
            max_pixels: 1024 * 1024,
        }
    }
}

pub struct Flux2Klein {
    transformer: Flux2Transformer,
    prompt_embedder: Flux2PromptEmbedder,
    vae: Flux2Vae,
    device: Device,
}

impl Flux2Klein {
    pub async fn load(runtime: &RuntimeManager) -> Result<Self> {
        let paths = Flux2KleinPaths {
            transformer_gguf: runtime
                .downloads()
                .bundled_model(FLUX2_REPO, FLUX2_GGUF)
                .await?,
            vae_safetensors: runtime
                .downloads()
                .bundled_model(VAE_REPO, VAE_FILE)
                .await?,
        };
        Self::load_from_paths(paths)
    }

    pub fn load_from_paths(paths: Flux2KleinPaths) -> Result<Self> {
        validate_path(&paths.transformer_gguf, "Flux2 transformer GGUF")?;
        validate_path(&paths.vae_safetensors, "Flux2 VAE")?;

        let model_device = device(false)?;

        let transformer = Flux2Transformer::from_gguf(&paths.transformer_gguf, &model_device)
            .with_context(|| {
                format!(
                    "failed to load Flux2 transformer from {}",
                    paths.transformer_gguf.display()
                )
            })?;
        if transformer.in_channels() != 128 {
            bail!(
                "unsupported Flux2 input channel count {}, expected 128",
                transformer.in_channels()
            );
        }
        if precomputed::PROMPT_EMBED_DIM != transformer.context_in_dim() {
            bail!(
                "embedded Flux2 prompt has {} channels, expected {} for this transformer",
                precomputed::PROMPT_EMBED_DIM,
                transformer.context_in_dim()
            );
        }
        let prompt_embedder = Flux2PromptEmbedder::new(&model_device);
        let vae = loading::load_mmaped_safetensors_path_with_dtype(
            &paths.vae_safetensors,
            &model_device,
            vae_dtype(&model_device),
            |vb| Flux2Vae::new(vb),
        )
        .with_context(|| {
            format!(
                "failed to load Flux2 VAE from {}",
                paths.vae_safetensors.display()
            )
        })?;

        Ok(Self {
            transformer,
            prompt_embedder,
            vae,
            device: model_device,
        })
    }

    pub fn precompute_prompt_embeddings(&self) -> Result<()> {
        self.prompt_embedder.encode_prompt()?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub fn image_to_image(
        &self,
        image: &DynamicImage,
        options: &Flux2ImageToImageOptions,
    ) -> Result<DynamicImage> {
        self.image_to_image_with_reference(image, None, options)
    }

    #[instrument(level = "debug", skip_all)]
    pub fn image_to_image_with_reference(
        &self,
        image: &DynamicImage,
        reference_image: Option<&DynamicImage>,
        options: &Flux2ImageToImageOptions,
    ) -> Result<DynamicImage> {
        if options.strength <= 0.0 {
            return Ok(image.clone());
        }

        let _cuda_cleanup = CudaTemporaryMemoryCleanup::new(&self.device);
        let (latents, packed_h, packed_w, size) = {
            let (rgb, size) = prepare_rgb_image(image, options.max_pixels);
            let image_latents = self.encode_image_latents(&rgb)?;
            let (batch, channels, packed_h, packed_w) = image_latents.dims4()?;
            if batch != 1 || channels != 128 {
                bail!("unexpected Flux2 latent shape {:?}", image_latents.shape());
            }

            let prompt_embeddings = self.prompt_embedder.encode_prompt()?;
            let transformer_dtype = transformer_dtype(&self.device);
            let prompt_embeds = prompt_embeddings
                .prompt_embeds
                .to_device(&self.device)?
                .to_dtype(transformer_dtype)?;
            let text_ids = prompt_embeddings.text_ids.to_device(&self.device)?;

            let image_latents_packed = pack_latents(&image_latents)?;
            let latent_ids = prepare_latent_ids(1, packed_h, packed_w, 0, &self.device)?;
            let mut condition_latents = image_latents_packed.clone();
            let mut condition_ids = prepare_latent_ids(1, packed_h, packed_w, 10, &self.device)?;
            if let Some(reference_image) = reference_image {
                let reference_latents =
                    self.encode_reference_latents(reference_image, options.max_pixels)?;
                let (_, _, ref_h, ref_w) = reference_latents.dims4()?;
                condition_latents =
                    Tensor::cat(&[condition_latents, pack_latents(&reference_latents)?], 1)?;
                condition_ids = Tensor::cat(
                    &[
                        condition_ids,
                        prepare_latent_ids(1, ref_h, ref_w, 20, &self.device)?,
                    ],
                    1,
                )?;
            }
            let condition_latents = condition_latents.to_dtype(transformer_dtype)?;
            let img_ids = Tensor::cat(&[latent_ids, condition_ids], 1)?;

            let mut scheduler =
                FlowMatchScheduler::new(options.num_inference_steps, packed_h * packed_w);
            let timesteps = scheduler.timesteps().to_vec();
            let start_index = start_index_for_strength(timesteps.len(), options.strength);
            scheduler.set_step_index(start_index);
            let noise = Tensor::randn(0f32, 1f32, image_latents.shape(), &self.device)?;
            let initial_timestep = timesteps[start_index];
            let mut latents =
                pack_latents(&scheduler.scale_noise(&image_latents, initial_timestep, &noise)?)?;
            drop(image_latents_packed);
            drop(image_latents);
            drop(noise);

            for step_idx in start_index..timesteps.len() {
                let timestep = Tensor::from_vec(
                    vec![scheduler.timestep_for_model(step_idx) as f32],
                    (1,),
                    &self.device,
                )?;
                let latent_model_input = Tensor::cat(
                    &[
                        latents.to_dtype(transformer_dtype)?,
                        condition_latents.clone(),
                    ],
                    1,
                )?;
                let noise_pred = self.transformer.forward(
                    &latent_model_input,
                    &img_ids,
                    &prompt_embeds,
                    &text_ids,
                    &timestep,
                )?;
                drop(latent_model_input);
                drop(timestep);
                let noise_pred = noise_pred
                    .narrow(1, 0, latents.dim(1)?)?
                    .to_dtype(DType::F32)?;
                let next_latents = scheduler.step(&noise_pred, &latents)?;
                drop(noise_pred);
                let previous_latents = std::mem::replace(&mut latents, next_latents);
                drop(previous_latents);
            }

            (latents, packed_h, packed_w, size)
        };

        let rgb = self.decode_packed_latents(latents, packed_h, packed_w)?;
        let mut output = resize_back_if_needed(rgb, size);
        if image.color().has_alpha() {
            output = restore_original_alpha(output, image);
        }
        Ok(output)
    }

    #[instrument(level = "debug", skip_all)]
    pub fn inpaint(
        &self,
        image: &DynamicImage,
        mask: &DynamicImage,
        options: &Flux2InpaintOptions,
    ) -> Result<DynamicImage> {
        self.inpaint_with_reference(image, mask, None, options)
    }

    #[instrument(level = "debug", skip_all)]
    pub fn inpaint_with_reference(
        &self,
        image: &DynamicImage,
        mask: &DynamicImage,
        reference_image: Option<&DynamicImage>,
        options: &Flux2InpaintOptions,
    ) -> Result<DynamicImage> {
        self.inpaint_with_reference_and_composite_mask(image, mask, mask, reference_image, options)
    }

    /// Generate with a broad mask while compositing through a tighter one.
    ///
    /// FLUX benefits from regenerating the whole detected text region, but
    /// pasting that entire region back produces a flat, discoloured rectangle.
    /// A glyph-level composite mask keeps the generated cleanup only where the
    /// original lettering actually needs replacing.
    #[instrument(level = "debug", skip_all)]
    pub fn inpaint_with_reference_and_composite_mask(
        &self,
        image: &DynamicImage,
        generation_mask: &DynamicImage,
        composite_mask: &DynamicImage,
        reference_image: Option<&DynamicImage>,
        options: &Flux2InpaintOptions,
    ) -> Result<DynamicImage> {
        if image.dimensions() != generation_mask.dimensions()
            || image.dimensions() != composite_mask.dimensions()
        {
            bail!(
                "image/mask dimensions mismatch: image is {:?}, generation mask is {:?}, composite mask is {:?}",
                image.dimensions(),
                generation_mask.dimensions(),
                composite_mask.dimensions()
            );
        }
        if options.strength <= 0.0 {
            return Ok(image.clone());
        }

        let gray_mask = generation_mask.to_luma8();
        let plan = plan_inpaint_crops(
            &gray_mask,
            image.width(),
            image.height(),
            options.mask_padding,
        );
        if plan.is_empty() {
            return Ok(image.clone());
        }

        tracing::info!(
            crop_count = plan.len(),
            crop_pixels = plan
                .iter()
                .map(|bounds| u64::from(bounds.width) * u64::from(bounds.height))
                .sum::<u64>(),
            steps = options.num_inference_steps,
            strength = options.strength,
            "planned Flux2 inpaint crops"
        );

        let mut running_output = image.clone();
        for (crop_index, bounds) in plan.into_iter().enumerate() {
            let image_crop =
                running_output.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
            let generation_mask_crop =
                generation_mask.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
            let composite_mask_crop =
                composite_mask.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
            let generation_started = Instant::now();
            let generated = self.inpaint_full_frame(
                &image_crop,
                &generation_mask_crop,
                reference_image,
                options,
            )?;
            let generation_ms = generation_started.elapsed().as_millis();
            let composite_started = Instant::now();
            running_output =
                composite_inpaint_crop(&running_output, &generated, &composite_mask_crop, bounds)?;
            tracing::info!(
                crop_index,
                x = bounds.x,
                y = bounds.y,
                width = bounds.width,
                height = bounds.height,
                generation_ms,
                composite_ms = composite_started.elapsed().as_millis(),
                "completed Flux2 inpaint crop"
            );
        }
        Ok(running_output)
    }

    fn inpaint_full_frame(
        &self,
        image: &DynamicImage,
        mask: &DynamicImage,
        reference_image: Option<&DynamicImage>,
        options: &Flux2InpaintOptions,
    ) -> Result<DynamicImage> {
        let _cuda_cleanup = CudaTemporaryMemoryCleanup::new(&self.device);
        let (latents, packed_h, packed_w, size) = {
            let (rgb, size) = prepare_rgb_image(image, options.max_pixels);
            let resized_mask = expand_mask(
                &prepare_mask(mask, size.width, size.height),
                options.mask_padding,
            );
            let image_latents = self.encode_image_latents(&rgb)?;
            let (batch, channels, packed_h, packed_w) = image_latents.dims4()?;
            if batch != 1 || channels != 128 {
                bail!("unexpected Flux2 latent shape {:?}", image_latents.shape());
            }
            let prompt_embeddings = self.prompt_embedder.encode_prompt()?;
            let transformer_dtype = transformer_dtype(&self.device);
            let prompt_embeds = prompt_embeddings
                .prompt_embeds
                .to_device(&self.device)?
                .to_dtype(transformer_dtype)?;
            let text_ids = prompt_embeddings.text_ids.to_device(&self.device)?;
            let latent_mask =
                mask_to_packed_tensor(&resized_mask, packed_h, packed_w, &self.device)?;

            let image_latents_packed = pack_latents(&image_latents)?;
            let latent_ids = prepare_latent_ids(1, packed_h, packed_w, 0, &self.device)?;
            let mut condition_latents = image_latents_packed.clone();
            let mut condition_ids = prepare_latent_ids(1, packed_h, packed_w, 10, &self.device)?;
            if let Some(reference_image) = reference_image {
                let reference_latents =
                    self.encode_reference_latents(reference_image, options.max_pixels)?;
                let (_, _, ref_h, ref_w) = reference_latents.dims4()?;
                condition_latents =
                    Tensor::cat(&[condition_latents, pack_latents(&reference_latents)?], 1)?;
                condition_ids = Tensor::cat(
                    &[
                        condition_ids,
                        prepare_latent_ids(1, ref_h, ref_w, 20, &self.device)?,
                    ],
                    1,
                )?;
            }
            let condition_latents = condition_latents.to_dtype(transformer_dtype)?;
            let img_ids = Tensor::cat(&[latent_ids, condition_ids], 1)?;

            let mut scheduler =
                FlowMatchScheduler::new(options.num_inference_steps, packed_h * packed_w);
            let timesteps = scheduler.timesteps().to_vec();
            let start_index = start_index_for_strength(timesteps.len(), options.strength);
            scheduler.set_step_index(start_index);
            let noise = Tensor::randn(0f32, 1f32, image_latents.shape(), &self.device)?;
            let noise_packed = pack_latents(&noise)?;
            let initial_timestep = timesteps[start_index];
            let mut latents =
                pack_latents(&scheduler.scale_noise(&image_latents, initial_timestep, &noise)?)?;
            let keep_mask = ((&latent_mask * -1.0)? + 1.0)?;
            drop(noise);

            for step_idx in start_index..timesteps.len() {
                let timestep = Tensor::from_vec(
                    vec![scheduler.timestep_for_model(step_idx) as f32],
                    (1,),
                    &self.device,
                )?;
                let latent_model_input = Tensor::cat(
                    &[
                        latents.to_dtype(transformer_dtype)?,
                        condition_latents.clone(),
                    ],
                    1,
                )?;
                let noise_pred = self.transformer.forward(
                    &latent_model_input,
                    &img_ids,
                    &prompt_embeds,
                    &text_ids,
                    &timestep,
                )?;
                drop(latent_model_input);
                drop(timestep);
                let noise_pred = noise_pred
                    .narrow(1, 0, latents.dim(1)?)?
                    .to_dtype(DType::F32)?;
                let next_latents = scheduler.step(&noise_pred, &latents)?;
                drop(noise_pred);
                let previous_latents = std::mem::replace(&mut latents, next_latents);
                drop(previous_latents);

                let init_latents = if step_idx + 1 < timesteps.len() {
                    scheduler.scale_noise(
                        &image_latents_packed,
                        timesteps[step_idx + 1],
                        &noise_packed,
                    )?
                } else {
                    image_latents_packed.clone()
                };
                let masked_latents = (keep_mask.broadcast_mul(&init_latents)?
                    + latent_mask.broadcast_mul(&latents)?)?;
                drop(init_latents);
                let previous_latents = std::mem::replace(&mut latents, masked_latents);
                drop(previous_latents);
            }

            (latents, packed_h, packed_w, size)
        };

        let rgb = self.decode_packed_latents(latents, packed_h, packed_w)?;
        let mut output = resize_back_if_needed(rgb, size);
        if image.color().has_alpha() {
            let original_alpha = inpainting::extract_alpha(&image.to_rgba8());
            let binary_mask = inpainting::binarize_mask(mask);
            let restored =
                inpainting::restore_alpha_channel(&output.to_rgb8(), &original_alpha, &binary_mask);
            output = DynamicImage::ImageRgba8(restored);
        }
        Ok(output)
    }

    fn encode_reference_latents(&self, image: &DynamicImage, max_pixels: u32) -> Result<Tensor> {
        let (rgb, _) = prepare_rgb_image(image, max_pixels);
        self.encode_image_latents(&rgb)
    }

    fn encode_image_latents(&self, image: &RgbImage) -> Result<Tensor> {
        let image_tensor =
            image_to_tensor(image, &self.device)?.to_dtype(vae_dtype(&self.device))?;
        let latents = self.vae.encode_patchified_normalized(&image_tensor)?;
        drop(image_tensor);
        Ok(latents.to_dtype(DType::F32)?)
    }

    fn decode_packed_latents(
        &self,
        packed_latents: Tensor,
        packed_h: usize,
        packed_w: usize,
    ) -> Result<RgbImage> {
        let patchified = unpack_latents(&packed_latents, packed_h, packed_w)?
            .to_dtype(vae_dtype(&self.device))?;
        drop(packed_latents);
        let decoded = self.vae.decode_patchified_normalized(&patchified)?;
        drop(patchified);
        let rgb = tensor_to_rgb_image(&decoded)?;
        drop(decoded);
        Ok(rgb)
    }
}

struct CudaTemporaryMemoryCleanup<'a> {
    device: &'a Device,
}

impl<'a> CudaTemporaryMemoryCleanup<'a> {
    fn new(device: &'a Device) -> Self {
        Self { device }
    }
}

impl Drop for CudaTemporaryMemoryCleanup<'_> {
    fn drop(&mut self) {
        let _ = release_cuda_temporary_memory(self.device);
    }
}

fn transformer_dtype(device: &Device) -> DType {
    if device.is_cuda() {
        return DType::BF16;
    }

    DType::F32
}

fn vae_dtype(device: &Device) -> DType {
    if device.is_cuda() {
        return DType::BF16;
    }

    DType::F32
}

fn release_cuda_temporary_memory(device: &Device) -> Result<()> {
    let started = Instant::now();
    device.synchronize()?;

    #[cfg(feature = "cuda")]
    if let Ok(cuda_device) = device.as_cuda_device() {
        let stream = cuda_device.cuda_stream();
        let context = stream.context();
        if context.has_async_alloc() {
            context.bind_to_thread()?;
            let pool = unsafe {
                candle_core::cuda::cudarc::driver::result::device::get_mem_pool(
                    context.cu_device(),
                )?
            };
            unsafe {
                candle_core::cuda::cudarc::driver::result::mem_pool::trim_to(pool, 0)?;
            }
        }
    }

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis(),
        "released Flux2 CUDA temporary memory"
    );
    Ok(())
}

fn mask_component_bounds(mask: &GrayImage) -> Vec<RawBounds> {
    // `connected_components` connects equal-valued pixels, so normalize every
    // nonzero mask value first to preserve the inpainter's nonzero-is-masked semantics.
    let binary = GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([u8::from(mask.get_pixel(x, y).0[0] != 0) * 255])
    });
    let labels = connected_components(&binary, Connectivity::Eight, Luma([0]));
    let component_count = labels.pixels().map(|pixel| pixel.0[0]).max().unwrap_or(0);
    let mut bounds: Vec<Option<RawBounds>> = vec![None; component_count as usize];

    for (x, y, pixel) in labels.enumerate_pixels() {
        let label = pixel.0[0];
        if label == 0 {
            continue;
        }
        let entry = &mut bounds[(label - 1) as usize];
        match entry {
            Some(bounds) => {
                bounds.min_x = bounds.min_x.min(x);
                bounds.min_y = bounds.min_y.min(y);
                bounds.max_x = bounds.max_x.max(x);
                bounds.max_y = bounds.max_y.max(y);
            }
            None => {
                *entry = Some(RawBounds {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                });
            }
        }
    }

    bounds.into_iter().flatten().collect()
}

fn padded_snapped_bounds(
    raw: RawBounds,
    image_w: u32,
    image_h: u32,
    mask_padding: u8,
) -> CropBounds {
    let padding = INPAINT_CROP_CONTEXT.max(mask_padding as u32);
    let multiple = IMAGE_MULTIPLE;
    let mut x0 = raw.min_x.saturating_sub(padding);
    let mut y0 = raw.min_y.saturating_sub(padding);
    let mut x1 = raw
        .max_x
        .saturating_add(1)
        .saturating_add(padding)
        .min(image_w);
    let mut y1 = raw
        .max_y
        .saturating_add(1)
        .saturating_add(padding)
        .min(image_h);

    x0 = (x0 / multiple) * multiple;
    y0 = (y0 / multiple) * multiple;
    x1 = x1.div_ceil(multiple) * multiple;
    y1 = y1.div_ceil(multiple) * multiple;
    x1 = x1.min(image_w);
    y1 = y1.min(image_h);

    CropBounds {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

fn merge_overlapping_crops(mut crops: Vec<CropBounds>) -> Vec<CropBounds> {
    let mut changed = true;
    while changed {
        changed = false;
        'search: for i in 0..crops.len() {
            for j in (i + 1)..crops.len() {
                if !crops_overlap(crops[i], crops[j]) {
                    continue;
                }

                crops[i] = crop_union(crops[i], crops[j]);
                crops.remove(j);
                changed = true;
                break 'search;
            }
        }
    }

    crops.sort_by_key(|bounds| (bounds.y, bounds.x));
    crops
}

fn crops_overlap(a: CropBounds, b: CropBounds) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

fn crop_union(a: CropBounds, b: CropBounds) -> CropBounds {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let x1 = (a.x + a.width).max(b.x + b.width);
    let y1 = (a.y + a.height).max(b.y + b.height);

    // Each coordinate is either IMAGE_MULTIPLE-aligned or clamped to an image edge;
    // choosing minima/maxima from those coordinates preserves the same invariant.
    CropBounds {
        x,
        y,
        width: x1 - x,
        height: y1 - y,
    }
}

fn plan_inpaint_crops(
    mask: &GrayImage,
    image_w: u32,
    image_h: u32,
    mask_padding: u8,
) -> Vec<CropBounds> {
    let crops = mask_component_bounds(mask)
        .into_iter()
        .map(|raw| padded_snapped_bounds(raw, image_w, image_h, mask_padding))
        .collect();
    merge_overlapping_crops(crops)
}

fn composite_inpaint_crop(
    original: &DynamicImage,
    generated_crop: &DynamicImage,
    mask_crop: &DynamicImage,
    bounds: CropBounds,
) -> Result<DynamicImage> {
    if generated_crop.dimensions() != (bounds.width, bounds.height) {
        bail!(
            "generated crop dimensions mismatch: got {:?}, expected {}x{}",
            generated_crop.dimensions(),
            bounds.width,
            bounds.height
        );
    }

    let mut output = original.to_rgba8();
    let generated = generated_crop.to_rgba8();
    let mask = mask_crop.to_luma8();
    let color_offsets = boundary_colour_offset_field(&output, &generated, &mask, bounds);
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let alpha = mask.get_pixel(x, y).0[0] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let generated_pixel = generated.get_pixel(x, y).0;
            let output_pixel = output.get_pixel_mut(bounds.x + x, bounds.y + y);
            let color_offset = color_offsets[(y * bounds.width + x) as usize];
            for (channel, generated_channel) in generated_pixel.iter().enumerate().take(3) {
                let matched_channel =
                    (f32::from(*generated_channel) + color_offset[channel]).clamp(0.0, 255.0);
                output_pixel.0[channel] = (output_pixel.0[channel] as f32 * (1.0 - alpha)
                    + matched_channel * alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }

    if original.color().has_alpha() {
        Ok(DynamicImage::ImageRgba8(output))
    } else {
        Ok(DynamicImage::ImageRgb8(
            DynamicImage::ImageRgba8(output).to_rgb8(),
        ))
    }
}

/// Estimate FLUX's low-frequency RGB bias from the narrow ring just outside
/// the pixels that will be pasted, then fit a smooth, robust affine offset through the erase mask.
/// Local drawing edges are not colour bias and must not become directional
/// streaks inside the reconstructed background.
fn boundary_colour_offset_field(
    original: &RgbaImage,
    generated: &RgbaImage,
    mask: &GrayImage,
    bounds: CropBounds,
) -> Vec<[f32; 3]> {
    let binary = GrayImage::from_fn(mask.width(), mask.height(), |x, y| {
        Luma([if mask.get_pixel(x, y).0[0] > 0 {
            255
        } else {
            0
        }])
    });
    let dilated = dilate(&binary, Norm::LInf, COLOR_MATCH_RING_RADIUS);
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    let mut ring_samples = Vec::new();

    for y in 0..bounds.height {
        for x in 0..bounds.width {
            if binary.get_pixel(x, y).0[0] > 0 || dilated.get_pixel(x, y).0[0] == 0 {
                continue;
            }
            let original_pixel = original.get_pixel(bounds.x + x, bounds.y + y).0;
            let generated_pixel = generated.get_pixel(x, y).0;
            let delta = [
                i16::from(original_pixel[0]) - i16::from(generated_pixel[0]),
                i16::from(original_pixel[1]) - i16::from(generated_pixel[1]),
                i16::from(original_pixel[2]) - i16::from(generated_pixel[2]),
            ];
            if delta
                .iter()
                .any(|value| value.abs() > MAX_COLOR_MATCH_SAMPLE_DELTA)
            {
                continue;
            }
            for channel in 0..3 {
                samples[channel].push(delta[channel]);
            }
            ring_samples.push((x, y, delta));
        }
    }

    let fallback: [f32; 3] =
        std::array::from_fn(|channel| f32::from(robust_colour_offset(&mut samples[channel])));
    let pixel_count = (bounds.width * bounds.height) as usize;
    if ring_samples.len() < MIN_COLOR_MATCH_SAMPLES {
        return vec![fallback; pixel_count];
    }

    // A colour bias varies smoothly. Drawing edges in the sample ring must
    // not be projected along individual rows/columns into the erased text.
    // Fit one robust affine field instead, rejecting local texture outliers.
    let observations: Vec<([f64; 3], [f64; 3])> = ring_samples
        .iter()
        .map(|&(x, y, delta)| {
            (
                [
                    1.0,
                    normalized_coordinate(x, bounds.width),
                    normalized_coordinate(y, bounds.height),
                ],
                delta.map(f64::from),
            )
        })
        .collect();
    let Some(coefficients) = fit_colour_plane(&observations) else {
        return vec![fallback; pixel_count];
    };
    let field = (0..pixel_count)
        .map(|index| {
            let x = index as u32 % bounds.width;
            let y = index as u32 / bounds.width;
            let basis = [
                1.0,
                normalized_coordinate(x, bounds.width),
                normalized_coordinate(y, bounds.height),
            ];
            std::array::from_fn(|channel| {
                (0..3)
                    .map(|i| basis[i] * coefficients[i][channel])
                    .sum::<f64>()
                    .clamp(
                        -f64::from(MAX_COLOR_MATCH_OFFSET),
                        f64::from(MAX_COLOR_MATCH_OFFSET),
                    ) as f32
            })
        })
        .collect();
    refine_local_colour_offsets(field, &ring_samples, bounds)
}

/// Match gradual, non-linear background changes that a single plane cannot
/// describe. Median samples on an eight-pixel grid reject isolated edges;
/// residuals far from the robust trend are excluded as structural changes.
/// Harmonic interpolation spreads the remaining bias smoothly in two
/// dimensions. The bounded coarse-grid solve avoids full-resolution diffusion
/// and bilinear sampling avoids the old row/column discontinuities.
fn refine_local_colour_offsets(
    mut field: Vec<[f32; 3]>,
    samples: &[(u32, u32, [i16; 3])],
    bounds: CropBounds,
) -> Vec<[f32; 3]> {
    const STEP: u32 = 8;
    let width = bounds.width.div_ceil(STEP) as usize;
    let height = bounds.height.div_ceil(STEP) as usize;
    let mut cells: Vec<Vec<[f32; 3]>> = vec![Vec::new(); width * height];
    for &(x, y, delta) in samples {
        let trend = field[(y * bounds.width + x) as usize];
        let residual = std::array::from_fn(|c| f32::from(delta[c]) - trend[c]);
        if residual.iter().all(|d| d.abs() <= 24.0) {
            cells[(y / STEP) as usize * width + (x / STEP) as usize].push(residual);
        }
    }
    let mut anchor = vec![false; width * height];
    let mut values = vec![[0.0f32; 3]; width * height];
    for (i, cell) in cells.iter_mut().enumerate() {
        if cell.len() < 4 {
            continue;
        }
        anchor[i] = true;
        values[i] = std::array::from_fn(|c| {
            cell.sort_unstable_by(|a, b| a[c].total_cmp(&b[c]));
            cell[cell.len() / 2][c]
        });
    }
    for _ in 0..160 {
        let mut change = 0.0f32;
        for parity in 0..2 {
            for y in 0..height {
                for x in 0..width {
                    let i = y * width + x;
                    if anchor[i] || (x + y) % 2 != parity {
                        continue;
                    }
                    let neighbours = [
                        y * width + x.saturating_sub(1),
                        y * width + (x + 1).min(width - 1),
                        y.saturating_sub(1) * width + x,
                        (y + 1).min(height - 1) * width + x,
                    ];
                    let next: [f32; 3] = std::array::from_fn(|c| {
                        neighbours.iter().map(|&j| values[j][c]).sum::<f32>() * 0.25
                    });
                    for c in 0..3 {
                        change = change.max((values[i][c] - next[c]).abs());
                    }
                    values[i] = next;
                }
            }
        }
        if change < 0.02 {
            break;
        }
    }
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let gx = (x as f32 / STEP as f32 - 0.5).max(0.0);
            let gy = (y as f32 / STEP as f32 - 0.5).max(0.0);
            let x0 = (gx.floor() as usize).min(width - 1);
            let y0 = (gy.floor() as usize).min(height - 1);
            let x1 = (x0 + 1).min(width - 1);
            let y1 = (y0 + 1).min(height - 1);
            let tx = gx - gx.floor();
            let ty = gy - gy.floor();
            let p = &mut field[(y * bounds.width + x) as usize];
            for c in 0..3 {
                let a = values[y0 * width + x0][c] * (1.0 - tx) + values[y0 * width + x1][c] * tx;
                let b = values[y1 * width + x0][c] * (1.0 - tx) + values[y1 * width + x1][c] * tx;
                p[c] = (p[c] + a * (1.0 - ty) + b * ty).clamp(-64.0, 64.0);
            }
        }
    }
    field
}

fn normalized_coordinate(value: u32, size: u32) -> f64 {
    2.0 * f64::from(value) / f64::from(size.saturating_sub(1).max(1)) - 1.0
}

/// Least squares with repeated residual trimming. A gradient is retained;
/// isolated line art is rejected. Highly inconsistent samples indicate that
/// this is structural reconstruction rather than a colour bias: leave the
/// generated colours alone in that case.
fn fit_colour_plane(observations: &[([f64; 3], [f64; 3])]) -> Option<[[f64; 3]; 3]> {
    let mut keep = vec![true; observations.len()];
    let mut coefficients = [[0.0; 3]; 3];
    for iteration in 0..5 {
        let mut matrix = [[0.0; 6]; 3];
        let mut count = 0;
        for ((basis, delta), accepted) in observations.iter().zip(&keep) {
            if !accepted {
                continue;
            }
            count += 1;
            for row in 0..3 {
                for col in 0..3 {
                    matrix[row][col] += basis[row] * basis[col];
                    matrix[row][col + 3] += basis[row] * delta[col];
                }
            }
        }
        if count < MIN_COLOR_MATCH_SAMPLES {
            return None;
        }
        // Pivoted elimination, shared by all three colour channels.
        for col in 0..3 {
            let pivot =
                (col..3).max_by(|&a, &b| matrix[a][col].abs().total_cmp(&matrix[b][col].abs()))?;
            if matrix[pivot][col].abs() < 1e-8 {
                return None;
            }
            matrix.swap(col, pivot);
            let scale = matrix[col][col];
            for j in col..6 {
                matrix[col][j] /= scale;
            }
            for row in 0..3 {
                if row == col {
                    continue;
                }
                let factor = matrix[row][col];
                for j in col..6 {
                    matrix[row][j] -= factor * matrix[col][j];
                }
            }
        }
        coefficients =
            std::array::from_fn(|row| std::array::from_fn(|channel| matrix[row][channel + 3]));
        let residuals: Vec<f64> = observations
            .iter()
            .map(|(basis, delta)| {
                (0..3)
                    .map(|channel| {
                        let predicted: f64 =
                            (0..3).map(|i| basis[i] * coefficients[i][channel]).sum();
                        (delta[channel] - predicted).abs()
                    })
                    .fold(0.0, f64::max)
            })
            .collect();
        let mut ordered = residuals.clone();
        ordered.sort_unstable_by(f64::total_cmp);
        let median = ordered[ordered.len() / 2];
        if iteration == 4 {
            if median > 12.0 || count * 5 < observations.len() * 3 {
                return Some([[0.0; 3]; 3]);
            }
        } else {
            let cutoff = (median * 4.0).clamp(3.0, 20.0);
            keep = residuals
                .iter()
                .map(|&residual| residual <= cutoff)
                .collect();
        }
    }
    Some(coefficients)
}

fn robust_colour_offset(samples: &mut [i16]) -> i16 {
    if samples.len() < MIN_COLOR_MATCH_SAMPLES {
        return 0;
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    let mut deviations = samples
        .iter()
        .map(|sample| (*sample - median).abs())
        .collect::<Vec<_>>();
    deviations.sort_unstable();
    if deviations[deviations.len() / 2] > MAX_COLOR_MATCH_MEDIAN_DEVIATION {
        return 0;
    }
    median.clamp(-MAX_COLOR_MATCH_OFFSET, MAX_COLOR_MATCH_OFFSET)
}

fn start_index_for_strength(num_steps: usize, strength: f64) -> usize {
    let strength = strength.clamp(0.0, 1.0);
    let init_timestep = ((num_steps as f64) * strength).round() as usize;
    num_steps.saturating_sub(init_timestep.max(1))
}

fn validate_path(path: &Path, label: &str) -> Result<()> {
    if !path.exists() {
        bail!("{label} path does not exist: {}", path.display());
    }
    Ok(())
}

fn restore_original_alpha(output: DynamicImage, original: &DynamicImage) -> DynamicImage {
    let mut rgba = output.to_rgba8();
    let alpha = inpainting::extract_alpha(&original.to_rgba8());
    for (x, y, pixel) in rgba.enumerate_pixels_mut() {
        pixel.0[3] = alpha.get_pixel(x, y).0[0];
    }
    DynamicImage::ImageRgba8(rgba)
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// Opt-in GPU audit on copied PNG fixtures. Retains raw generated crops
    /// so compositor comparisons can use exactly the same model output.
    #[test]
    #[ignore = "requires copied PNG fixtures and explicit local Flux2 model paths"]
    fn retained_inpaint_crops() -> Result<()> {
        let dir = PathBuf::from(std::env::var("KOHARU_INPAINT_QA")?);
        let model = Flux2Klein::load_from_paths(Flux2KleinPaths {
            transformer_gguf: PathBuf::from(std::env::var("KOHARU_FLUX_TRANSFORMER")?),
            vae_safetensors: PathBuf::from(std::env::var("KOHARU_FLUX_VAE")?),
        })?;
        let options = Flux2InpaintOptions {
            num_inference_steps: 2,
            ..Default::default()
        };
        for tag in ["011", "013"] {
            let source = image::open(dir.join(format!("{tag}-source.png")))?;
            let generation = image::open(dir.join(format!("{tag}-generation.png")))?;
            let composite = image::open(dir.join(format!("{tag}-composite.png")))?;
            let plan = plan_inpaint_crops(
                &generation.to_luma8(),
                source.width(),
                source.height(),
                options.mask_padding,
            );
            let mut output = source.clone();
            let mut bounds_json = Vec::new();
            for (index, bounds) in plan.into_iter().enumerate() {
                let crop = output.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
                let gen_mask = generation.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
                let paste_mask =
                    composite.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
                let generated = model.inpaint_full_frame(&crop, &gen_mask, None, &options)?;
                let name = format!("{tag}-crop-{index}");
                crop.save(dir.join(format!("{name}-source.png")))?;
                generated.save(dir.join(format!("{name}-generated.png")))?;
                paste_mask.save(dir.join(format!("{name}-paste.png")))?;
                output = composite_inpaint_crop(&output, &generated, &paste_mask, bounds)?;
                bounds_json.push(serde_json::json!({"index": index, "x": bounds.x, "y": bounds.y, "width": bounds.width, "height": bounds.height}));
                println!("saved {name} raw output");
            }
            output.save(dir.join(format!("{tag}-fixed.png")))?;
            std::fs::write(
                dir.join(format!("{tag}-crops.json")),
                serde_json::to_vec_pretty(&bounds_json)?,
            )?;
            let original = source.to_rgba8();
            let result = output.to_rgba8();
            let mask = composite.to_luma8();
            for (x, y, pixel) in original.enumerate_pixels() {
                if mask.get_pixel(x, y)[0] == 0 {
                    assert_eq!(pixel, result.get_pixel(x, y));
                }
            }
        }
        Ok(())
    }

    fn crop(x: u32, y: u32, width: u32, height: u32) -> CropBounds {
        CropBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn component_bounds_find_separated_blobs() {
        let mut mask = GrayImage::new(12, 10);
        for y in 2..=4 {
            for x in 1..=3 {
                mask.put_pixel(x, y, Luma([127]));
            }
        }
        for y in 6..=8 {
            for x in 8..=9 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }

        assert_eq!(
            mask_component_bounds(&mask),
            vec![
                RawBounds {
                    min_x: 1,
                    min_y: 2,
                    max_x: 3,
                    max_y: 4,
                },
                RawBounds {
                    min_x: 8,
                    min_y: 6,
                    max_x: 9,
                    max_y: 8,
                },
            ]
        );
    }

    #[test]
    fn component_bounds_use_eight_connectivity() {
        let mut mask = GrayImage::new(6, 6);
        mask.put_pixel(2, 2, Luma([64]));
        mask.put_pixel(3, 3, Luma([255]));

        assert_eq!(
            mask_component_bounds(&mask),
            vec![RawBounds {
                min_x: 2,
                min_y: 2,
                max_x: 3,
                max_y: 3,
            }]
        );
    }

    #[test]
    fn component_bounds_are_empty_for_empty_mask() {
        assert!(mask_component_bounds(&GrayImage::new(8, 8)).is_empty());
    }

    #[test]
    fn padded_bounds_respect_padding_and_flux_alignment() {
        let raw = RawBounds {
            min_x: 100,
            min_y: 70,
            max_x: 130,
            max_y: 90,
        };
        let bounds = padded_snapped_bounds(raw, 512, 400, 16);

        assert_eq!(bounds, crop(32, 0, 176, 160));
        assert_eq!(bounds.x % IMAGE_MULTIPLE, 0);
        assert_eq!(bounds.y % IMAGE_MULTIPLE, 0);
        assert_eq!((bounds.x + bounds.width) % IMAGE_MULTIPLE, 0);
        assert_eq!((bounds.y + bounds.height) % IMAGE_MULTIPLE, 0);
        assert!(bounds.x <= raw.min_x - INPAINT_CROP_CONTEXT);
        assert!(bounds.y <= raw.min_y - INPAINT_CROP_CONTEXT);
        assert!(bounds.x + bounds.width >= raw.max_x + 1 + INPAINT_CROP_CONTEXT);
        assert!(bounds.y + bounds.height >= raw.max_y + 1 + INPAINT_CROP_CONTEXT);
    }

    #[test]
    fn padded_bounds_allow_full_frame_crop() {
        let raw = RawBounds {
            min_x: 0,
            min_y: 0,
            max_x: 255,
            max_y: 255,
        };

        assert_eq!(
            padded_snapped_bounds(raw, 256, 256, 16),
            crop(0, 0, 256, 256)
        );
    }

    #[test]
    fn padded_bounds_clamp_non_aligned_image_edges() {
        let raw = RawBounds {
            min_x: 230,
            min_y: 220,
            max_x: 249,
            max_y: 237,
        };
        let bounds = padded_snapped_bounds(raw, 250, 238, 16);

        assert_eq!(bounds, crop(160, 144, 90, 94));
        assert_eq!(bounds.x % IMAGE_MULTIPLE, 0);
        assert_eq!(bounds.y % IMAGE_MULTIPLE, 0);
        assert_eq!(bounds.x + bounds.width, 250);
        assert_eq!(bounds.y + bounds.height, 238);
    }

    #[test]
    fn merge_overlapping_pair_returns_union() {
        assert_eq!(
            merge_overlapping_crops(vec![crop(0, 0, 96, 96), crop(64, 64, 96, 96)]),
            vec![crop(0, 0, 160, 160)]
        );
    }

    #[test]
    fn merge_disjoint_pair_keeps_both() {
        assert_eq!(
            merge_overlapping_crops(vec![crop(0, 0, 32, 32), crop(64, 64, 32, 32)]),
            vec![crop(0, 0, 32, 32), crop(64, 64, 32, 32)]
        );
    }

    #[test]
    fn merge_overlapping_crops_reaches_transitive_fixpoint() {
        let horizontal = crop(0, 0, 64, 16);
        let vertical = crop(48, 0, 16, 64);
        let newly_intersecting = crop(0, 48, 16, 16);

        assert_eq!(
            merge_overlapping_crops(vec![newly_intersecting, vertical, horizontal]),
            vec![crop(0, 0, 64, 64)]
        );
    }

    #[test]
    fn merge_disjoint_output_is_sorted_by_y_then_x() {
        assert_eq!(
            merge_overlapping_crops(vec![
                crop(96, 64, 16, 16),
                crop(64, 0, 16, 16),
                crop(16, 0, 16, 16),
            ]),
            vec![
                crop(16, 0, 16, 16),
                crop(64, 0, 16, 16),
                crop(96, 64, 16, 16),
            ]
        );
    }

    #[test]
    fn crop_plan_is_empty_for_empty_mask() {
        assert!(plan_inpaint_crops(&GrayImage::new(512, 512), 512, 512, 16).is_empty());
    }

    #[test]
    fn crop_plan_keeps_far_apart_bubbles_small_and_separate() {
        let mut mask = GrayImage::new(1024, 1024);
        for y in 100..=120 {
            for x in 100..=120 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        for y in 800..=820 {
            for x in 800..=820 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }

        let plan = plan_inpaint_crops(&mask, 1024, 1024, 16);

        assert_eq!(plan.len(), 2);
        for bounds in plan {
            assert!(bounds.width < 1024 / 2);
            assert!(bounds.height < 1024 / 2);
            assert!(bounds.width * bounds.height < 1024 * 1024 / 4);
        }
    }

    #[test]
    fn crop_plan_merges_close_bubbles_after_padding() {
        let mut mask = GrayImage::new(512, 512);
        mask.put_pixel(100, 100, Luma([255]));
        mask.put_pixel(180, 100, Luma([255]));

        assert_eq!(
            plan_inpaint_crops(&mask, 512, 512, 16),
            vec![crop(32, 32, 224, 144)]
        );
    }

    #[test]
    fn composite_chaining_preserves_unmasked_pixels() {
        let base = DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 16, Rgb([10, 20, 30])));
        let generated_a = DynamicImage::ImageRgb8(RgbImage::from_pixel(16, 16, Rgb([200, 0, 0])));
        let generated_b = DynamicImage::ImageRgb8(RgbImage::from_pixel(16, 16, Rgb([0, 0, 200])));
        let mut mask_a = GrayImage::new(16, 16);
        mask_a.put_pixel(2, 3, Luma([255]));
        let mut mask_b = GrayImage::new(16, 16);
        mask_b.put_pixel(5, 7, Luma([255]));

        let first = composite_inpaint_crop(
            &base,
            &generated_a,
            &DynamicImage::ImageLuma8(mask_a),
            crop(0, 0, 16, 16),
        )
        .unwrap();
        let output = composite_inpaint_crop(
            &first,
            &generated_b,
            &DynamicImage::ImageLuma8(mask_b),
            crop(16, 0, 16, 16),
        )
        .unwrap()
        .to_rgb8();

        for (x, y, pixel) in output.enumerate_pixels() {
            let expected = match (x, y) {
                (2, 3) => Rgb([200, 0, 0]),
                (21, 7) => Rgb([0, 0, 200]),
                _ => Rgb([10, 20, 30]),
            };
            assert_eq!(*pixel, expected);
        }
    }

    #[test]
    fn composite_matches_generated_bias_from_outside_mask_without_edge_feather() {
        let mut base = RgbImage::from_pixel(24, 24, Rgb([100, 110, 120]));
        let generated = RgbImage::from_pixel(24, 24, Rgb([120, 130, 140]));
        let mut mask = GrayImage::new(24, 24);
        for y in 6..18 {
            for x in 6..18 {
                // Simulate bright source lettering under the erase mask. It
                // must not participate in colour matching or survive at edge.
                base.put_pixel(x, y, Rgb([240, 240, 240]));
                mask.put_pixel(x, y, Luma([255]));
            }
        }

        let output = composite_inpaint_crop(
            &DynamicImage::ImageRgb8(base),
            &DynamicImage::ImageRgb8(generated),
            &DynamicImage::ImageLuma8(mask),
            crop(0, 0, 24, 24),
        )
        .unwrap()
        .to_rgb8();

        assert_eq!(*output.get_pixel(6, 12), Rgb([100, 110, 120]));
        assert_eq!(*output.get_pixel(12, 12), Rgb([100, 110, 120]));
        assert_eq!(*output.get_pixel(5, 12), Rgb([100, 110, 120]));
    }

    #[test]
    fn composite_does_not_project_boundary_art_into_stripes() {
        let mut base = RgbImage::from_pixel(128, 128, Rgb([128; 3]));
        let generated = base.clone();
        let mut mask = GrayImage::new(128, 128);
        for y in 32..96 {
            for x in 32..96 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        // A small detail just outside the mask used to produce a stripe
        // reaching its centre (136 instead of the correct flat 128).
        for y in 45..49 {
            for x in 30..32 {
                base.put_pixel(x, y, Rgb([168; 3]));
            }
        }
        let output = composite_inpaint_crop(
            &DynamicImage::ImageRgb8(base.clone()),
            &DynamicImage::ImageRgb8(generated),
            &DynamicImage::ImageLuma8(mask.clone()),
            crop(0, 0, 128, 128),
        )
        .unwrap()
        .to_rgb8();
        for (x, y, pixel) in output.enumerate_pixels() {
            if mask.get_pixel(x, y)[0] == 0 {
                assert_eq!(pixel, base.get_pixel(x, y));
            } else {
                assert!(
                    (i16::from(pixel[0]) - 128).abs() <= 1,
                    "({x},{y}): {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn composite_matches_curved_background_bias_without_patch_edges() -> Result<()> {
        let background = |y: u32| (128.0 + 12.0 * (y as f32 / 20.0).sin()).round() as u8;
        let original =
            DynamicImage::ImageRgb8(RgbImage::from_fn(96, 96, |_, y| Rgb([background(y); 3])));
        let generated = DynamicImage::ImageRgb8(RgbImage::from_pixel(96, 96, Rgb([128; 3])));
        let mask = DynamicImage::ImageLuma8(GrayImage::from_fn(96, 96, |x, y| {
            Luma([if (40..56).contains(&x) && (8..88).contains(&y) {
                255
            } else {
                0
            }])
        }));
        let output =
            composite_inpaint_crop(&original, &generated, &mask, crop(0, 0, 96, 96))?.to_rgb8();
        for y in [24, 40, 72] {
            for x in [40, 48, 55] {
                assert!(
                    (i16::from(output.get_pixel(x, y)[0]) - i16::from(background(y))).abs() <= 2
                );
            }
        }
        Ok(())
    }

    #[test]
    fn composite_tracks_background_gradient_across_mask() {
        let mut base = RgbImage::new(48, 32);
        let mut generated = RgbImage::new(48, 32);
        let mut mask = GrayImage::new(48, 32);
        for y in 0..32 {
            for x in 0..48 {
                let background = 55 + x * 3;
                base.put_pixel(x, y, Rgb([background as u8; 3]));
                generated.put_pixel(x, y, Rgb([135; 3]));
                if (10..38).contains(&x) && (8..24).contains(&y) {
                    // The source pixels under the mask stand in for lettering;
                    // the expected clean background still follows the gradient.
                    base.put_pixel(x, y, Rgb([245; 3]));
                    mask.put_pixel(x, y, Luma([255]));
                }
            }
        }

        let output = composite_inpaint_crop(
            &DynamicImage::ImageRgb8(base),
            &DynamicImage::ImageRgb8(generated),
            &DynamicImage::ImageLuma8(mask),
            crop(0, 0, 48, 32),
        )
        .unwrap()
        .to_rgb8();

        for x in [10, 16, 24, 32, 37] {
            let expected = (55 + x * 3) as i16;
            let actual = i16::from(output.get_pixel(x, 16).0[0]);
            assert!(
                (actual - expected).abs() <= 4,
                "x={x}: expected {expected}, got {actual}"
            );
        }
        assert_eq!(*output.get_pixel(9, 16), Rgb([82; 3]));
        assert_eq!(*output.get_pixel(38, 16), Rgb([169; 3]));
    }
}
