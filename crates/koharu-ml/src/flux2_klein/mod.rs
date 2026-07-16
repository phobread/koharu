mod latents;
mod precomputed;
pub mod qwen;
mod scheduler;
mod transformer;
mod vae;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use candle_core::{DType, Device, Tensor};
use image::{DynamicImage, GenericImageView, GrayImage, Luma, RgbImage};
use imageproc::region_labelling::{Connectivity, connected_components};
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
                .huggingface_model(FLUX2_REPO, FLUX2_GGUF)
                .await?,
            vae_safetensors: runtime
                .downloads()
                .huggingface_model(VAE_REPO, VAE_FILE)
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
        if image.dimensions() != mask.dimensions() {
            bail!(
                "image/mask dimensions mismatch: image is {:?}, mask is {:?}",
                image.dimensions(),
                mask.dimensions()
            );
        }
        if options.strength <= 0.0 {
            return Ok(image.clone());
        }

        let gray_mask = mask.to_luma8();
        let plan = plan_inpaint_crops(
            &gray_mask,
            image.width(),
            image.height(),
            options.mask_padding,
        );
        if plan.is_empty() {
            return Ok(image.clone());
        }

        let mut running_output = image.clone();
        for bounds in plan {
            let image_crop =
                running_output.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
            let mask_crop = mask.crop_imm(bounds.x, bounds.y, bounds.width, bounds.height);
            let generated =
                self.inpaint_full_frame(&image_crop, &mask_crop, reference_image, options)?;
            running_output =
                composite_inpaint_crop(&running_output, &generated, &mask_crop, bounds)?;
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
    for y in 0..bounds.height {
        for x in 0..bounds.width {
            let alpha = mask.get_pixel(x, y).0[0] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }
            let generated_pixel = generated.get_pixel(x, y).0;
            let output_pixel = output.get_pixel_mut(bounds.x + x, bounds.y + y);
            for (channel, generated_channel) in generated_pixel.iter().enumerate().take(3) {
                output_pixel.0[channel] = (output_pixel.0[channel] as f32 * (1.0 - alpha)
                    + *generated_channel as f32 * alpha)
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
}
