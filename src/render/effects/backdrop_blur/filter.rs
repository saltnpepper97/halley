//! Separable Gaussian/box kernels. Large radii use the existing downsample
//! shader before filtering so each axis needs at most 17 texture reads.
use halley_config::{Blur, BlurMethod};
use smithay::backend::renderer::gles::{GlesError, ffi, link_program};

use super::{BLUR_VERTEX_SHADER, raw_fragment_shader};

const SHADER: &str = include_str!("../shaders/blur_filter.frag");
const MAX_RADIUS: f32 = 16.0;

#[derive(Clone, Copy)]
pub(super) struct Kernel {
    pub center: f32,
    pub pairs: [f32; 16],
}

impl Kernel {
    pub const IDENTITY: Self = Self {
        center: 1.0,
        pairs: [0.0; 16],
    };

    fn new(method: BlurMethod, radius: f32) -> Self {
        if radius <= f32::EPSILON {
            return Self::IDENTITY;
        }
        let mut weights = [0.0_f32; 17];
        weights[0] = 1.0;
        let sigma = radius / 3.0;
        for (i, weight) in weights
            .iter_mut()
            .enumerate()
            .skip(1)
            .take(radius.ceil() as usize)
        {
            *weight = match method {
                BlurMethod::Gaussian => (-0.5 * (i as f32 / sigma).powi(2)).exp(),
                BlurMethod::Box => (radius + 1.0 - i as f32).clamp(0.0, 1.0),
                BlurMethod::DualKawase => unreachable!("Kawase uses its existing shaders"),
            };
        }
        let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
        let mut kernel = Self {
            center: weights[0] / total,
            pairs: [0.0; 16],
        };
        for pair in 0..8 {
            let i = pair * 2 + 1;
            let weight = weights[i] + weights[i + 1];
            if weight > 0.0 {
                kernel.pairs[pair * 2] = i as f32 + weights[i + 1] / weight;
                kernel.pairs[pair * 2 + 1] = weight / total;
            }
        }
        kernel
    }
}

#[derive(Clone, Copy)]
pub(super) struct Settings {
    pub kernel: Kernel,
    pub iterations: u32,
    pub levels: u32,
}

impl Settings {
    pub fn new(config: Blur) -> Option<Self> {
        if config.method == BlurMethod::DualKawase {
            return None;
        }
        let iterations = config.passes.clamp(1, 5);
        // Repeated convolution adds variances, not radii. Dividing by sqrt(n)
        // keeps the overall spread approximately stable across pass counts.
        let mut radius = config.radius.clamp(0.0, 128.0) / (iterations as f32).sqrt();
        let mut levels = 0;
        while radius > MAX_RADIUS {
            radius *= 0.5;
            levels += 1;
        }
        Some(Self {
            kernel: Kernel::new(config.method, radius),
            iterations,
            levels,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct Program {
    pub program: ffi::types::GLuint,
    pub texture: ffi::types::GLint,
    pub step: ffi::types::GLint,
    pub center: ffi::types::GLint,
    pub kernel: ffi::types::GLint,
    pub vertex: ffi::types::GLint,
}

impl Program {
    pub unsafe fn compile(gl: &ffi::Gles2) -> Result<Self, GlesError> {
        let program =
            unsafe { link_program(gl, BLUR_VERTEX_SHADER, &raw_fragment_shader(SHADER))? };
        let pass = Self {
            program,
            texture: unsafe { gl.GetUniformLocation(program, c"tex".as_ptr()) },
            step: unsafe { gl.GetUniformLocation(program, c"pixel_step".as_ptr()) },
            center: unsafe { gl.GetUniformLocation(program, c"center_weight".as_ptr()) },
            kernel: unsafe { gl.GetUniformLocation(program, c"kernel[0]".as_ptr()) },
            vertex: unsafe { gl.GetAttribLocation(program, c"vert".as_ptr()) },
        };
        if [
            pass.texture,
            pass.step,
            pass.center,
            pass.kernel,
            pass.vertex,
        ]
        .into_iter()
        .any(|location| location < 0)
        {
            unsafe { gl.DeleteProgram(program) };
            return Err(GlesError::ShaderCompileError);
        }
        Ok(pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels_are_normalized_and_sampling_is_bounded() {
        for method in [BlurMethod::Gaussian, BlurMethod::Box] {
            for radius in [0.0, 0.1, 0.5, 1.0, 15.5, 16.0, 24.0, 128.0] {
                for passes in 1..=5 {
                    let settings = Settings::new(Blur {
                        method,
                        radius,
                        passes,
                        ..Default::default()
                    })
                    .unwrap();
                    let kernel = settings.kernel;
                    let sum = kernel.center
                        + 2.0 * kernel.pairs.chunks_exact(2).map(|p| p[1]).sum::<f32>();
                    assert!((sum - 1.0).abs() < 1e-6);
                    assert!(kernel.pairs.iter().all(|v| v.is_finite() && *v >= 0.0));
                    assert!(kernel.pairs.chunks_exact(2).all(|p| p[0] <= MAX_RADIUS));
                    assert!(settings.levels <= 3);
                }
            }
        }
    }
}
