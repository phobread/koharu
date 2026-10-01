mod hf_hub;

pub mod anime_text;
pub mod aot_inpainting;
pub mod comic_text_bubble_detector;
pub mod comic_text_detector;
pub mod flux2_klein;
pub mod font_detector;
pub mod inpainting;
pub mod korean_ocr;
pub mod lama;
pub mod loading;
pub mod manga_ocr;
pub mod manga_text_segmentation_2025;
pub mod mit48px_ocr;
mod ops;
pub mod outlined_text;
pub mod paddleocr_vl;
pub mod pp_doclayout_v3;
pub mod probability_map;
pub mod speech_bubble_segmentation;
pub mod types;

pub use types::{
    FontPrediction, NamedFontPrediction, Quad, TextDirection, TextRegion, TopFont, quad_bbox,
    rotated_box_corners,
};

use anyhow::Result;
use candle_core::utils::{cuda_is_available, metal_is_available};

pub use candle_core::Device;

static GPU_SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn device(cpu: bool) -> Result<Device> {
    if cpu {
        Ok(Device::Cpu)
    } else if cuda_is_available()
        && *GPU_SUPPORTED.get_or_init(koharu_runtime::check_cuda_driver_support)
    {
        Ok(Device::new_cuda(0)?)
    } else if metal_is_available() {
        Ok(Device::new_metal(0)?)
    } else {
        tracing::warn!(
            "No GPU support detected; falling back to CPU. For better performance, ensure you have a compatible NVIDIA GPU with the latest drivers, or a recent Apple device with Metal support."
        );
        Ok(Device::Cpu)
    }
}

/// Hand GPU memory freed by dropped models back to the driver.
///
/// candle allocates from CUDA's stream-ordered memory pool, which keeps freed
/// blocks reserved for this process. After unloading a model, trim the pool so
/// the next model (or llama.cpp's separate allocator) can actually use it.
pub fn release_gpu_memory() -> Result<()> {
    // Nothing to release unless a CUDA device was ever handed out.
    #[cfg(feature = "cuda")]
    if GPU_SUPPORTED.get().copied().unwrap_or(false) && cuda_is_available() {
        use candle_core::cuda::cudarc::driver::{CudaContext, result};

        let context = CudaContext::new(0)?;
        context.synchronize()?;
        if context.has_async_alloc() {
            unsafe {
                let pool = result::device::get_mem_pool(context.cu_device())?;
                result::mem_pool::trim_to(pool, 0)?;
            }
        }
    }
    Ok(())
}
