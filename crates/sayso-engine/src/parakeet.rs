//! The final pass of `parakeet_cpp` models: a Parakeet TDT GGUF on parakeet.cpp (ggml), on the GPU through
//! Vulkan. Without a discrete GPU such a model refuses to load. `build.rs` builds parakeet.cpp and links
//! it statically (feature `parakeet`, Linux); without the feature, loading one fails with a clear error.

use crate::recognizer::FinalPass;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// The load of a GPU model found no discrete GPU. The `reason` of the `failed` state is `no_gpu` for it.
#[derive(Debug)]
pub struct NoGpu;

impl std::fmt::Display for NoGpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this model needs a discrete GPU with a Vulkan driver, and none was found")
    }
}

impl std::error::Error for NoGpu {}

#[cfg(not(all(feature = "parakeet", target_os = "linux")))]
pub fn load(model: &Path) -> Result<Arc<dyn FinalPass>> {
    anyhow::bail!(
        "{} needs parakeet.cpp, and this engine was built without the parakeet feature",
        model.display()
    )
}

#[cfg(all(feature = "parakeet", target_os = "linux"))]
pub use imp::load;

#[cfg(all(feature = "parakeet", target_os = "linux"))]
mod imp {
    use super::*;
    use anyhow::{Context, anyhow};
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt as _;
    use std::sync::Mutex;

    const SAMPLE_RATE: c_int = 16_000;

    #[repr(C)]
    struct RawContext {
        _private: [u8; 0],
    }

    // parakeet.cpp's include/parakeet_capi.h, linked statically by build.rs.
    unsafe extern "C" {
        fn parakeet_capi_load(gguf_path: *const c_char) -> *mut RawContext;
        fn parakeet_capi_load_error() -> *const c_char;
        fn parakeet_capi_free(ctx: *mut RawContext);
        fn parakeet_capi_transcribe_pcm(
            ctx: *mut RawContext,
            samples: *const f32,
            n_samples: c_int,
            sample_rate: c_int,
            decoder: c_int,
        ) -> *mut c_char;
        fn parakeet_capi_last_error(ctx: *mut RawContext) -> *const c_char;
        fn parakeet_capi_free_string(s: *mut c_char);
        // ggml-backend.h, linked with it.
        fn ggml_backend_dev_count() -> usize;
        fn ggml_backend_dev_get(index: usize) -> *mut c_void;
        fn ggml_backend_dev_type(device: *mut c_void) -> c_int;
    }

    // `enum ggml_backend_dev_type`. ggml-vulkan lists an integrated GPU as IGPU and any other device as GPU.
    const GGML_BACKEND_DEVICE_TYPE_GPU: c_int = 1;
    const GGML_BACKEND_DEVICE_TYPE_IGPU: c_int = 2;

    /// The device parakeet.cpp picks, the first GPU or IGPU in ggml's list, is not an integrated GPU. The
    /// list follows the Vulkan device order, where Mesa can put a laptop's integrated GPU first; the app's
    /// `GGML_VK_VISIBLE_DEVICES` leaves only the discrete one in it.
    fn has_gpu() -> bool {
        // SAFETY: registry lookups with no preconditions, at indexes below the count.
        let first = (0..unsafe { ggml_backend_dev_count() })
            .map(|i| unsafe { ggml_backend_dev_type(ggml_backend_dev_get(i)) })
            .find(|t| matches!(*t, GGML_BACKEND_DEVICE_TYPE_GPU | GGML_BACKEND_DEVICE_TYPE_IGPU));
        first == Some(GGML_BACKEND_DEVICE_TYPE_GPU)
    }

    /// An owned parakeet.cpp context, behind a mutex: the library queues concurrent calls but keeps only
    /// the latest error, so the lock makes `last_error` this call's own.
    struct OwnedContext(*mut RawContext);

    // SAFETY: the context is only used under the mutex of `Gguf`, one call at a time, which is
    // what the C API allows from any thread.
    unsafe impl Send for OwnedContext {}

    impl Drop for OwnedContext {
        fn drop(&mut self) {
            // SAFETY: the pointer came from parakeet_capi_load and is freed once, here.
            unsafe { parakeet_capi_free(self.0) }
        }
    }

    struct Gguf {
        ctx: Mutex<OwnedContext>,
    }

    pub fn load(model: &Path) -> Result<Arc<dyn FinalPass>> {
        // parakeet.cpp would fall back to an integrated GPU (about 3 times slower than a sherpa-onnx model on
        // the CPU) or to the CPU (19 s for 58 s of speech): a sherpa-onnx model is the better choice there.
        if !has_gpu() {
            return Err(NoGpu.into());
        }
        let path = CString::new(model.as_os_str().as_bytes())
            .context("the model path has a NUL byte")?;
        // SAFETY: a valid C string; the result is NULL or a context we own.
        let raw = unsafe { parakeet_capi_load(path.as_ptr()) };
        if raw.is_null() {
            // SAFETY: never NULL, valid until the next load on this thread.
            let reason = unsafe { CStr::from_ptr(parakeet_capi_load_error()) };
            return Err(anyhow!("parakeet.cpp cannot load {}: {}", model.display(), reason.to_string_lossy()));
        }
        let gguf = Gguf {
            ctx: Mutex::new(OwnedContext(raw)),
        };
        // ggml-vulkan builds its GPU pipelines on first use, per kind of work, and NVIDIA's driver caches
        // them on disk. With the cache empty (after a driver update) a first 3 s dictation took 8 s; these
        // three lengths build every pipeline the model uses, so later first decodes cost what repeats do.
        for seconds in [1, 30, 60] {
            gguf.transcribe(&vec![0.0; SAMPLE_RATE as usize * seconds], "en")?;
        }
        Ok(Arc::new(gguf))
    }

    impl FinalPass for Gguf {
        fn transcribe(&self, samples: &[f32], _language: &str) -> Result<String> {
            let ctx = self.ctx.lock().unwrap_or_else(|e| e.into_inner());
            let n = c_int::try_from(samples.len()).context("the piece of audio is too long")?;
            // SAFETY: the context is live and locked; `samples` outlives the call. decoder 0 is the
            // model's own head, the transducer for TDT.
            let text = unsafe { parakeet_capi_transcribe_pcm(ctx.0, samples.as_ptr(), n, SAMPLE_RATE, 0) };
            if text.is_null() {
                // SAFETY: the context's last error, valid while we hold the lock.
                let reason = unsafe { CStr::from_ptr(parakeet_capi_last_error(ctx.0)) };
                return Err(anyhow!("parakeet.cpp: {}", reason.to_string_lossy()));
            }
            // SAFETY: a NUL-terminated string the library allocated; freed once, after the copy.
            let result = unsafe { CStr::from_ptr(text) }.to_string_lossy().trim().to_string();
            unsafe { parakeet_capi_free_string(text) };
            Ok(result)
        }
    }
}
