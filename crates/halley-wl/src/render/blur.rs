//! High-quality backdrop blur (Dual Kawase) for compositor overlays and
//! translucent client windows.
//!
//! The frame code captures the current output framebuffer immediately before a
//! blur-enabled window is drawn, blurs that capture into [`BlurTextures::result`],
//! and composites the matching patch before drawing the window itself. This keeps
//! backdrop blur z-ordered, so windows behind translucent windows are blurred too.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind, Color32F, Frame, FrameContext, Offscreen, Renderer, Texture,
            gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform},
        },
    },
    utils::{Buffer, Physical, Rectangle, Size, Transform},
};

use smithay::backend::renderer::gles::ffi;

/// Persistent offscreen textures for the blur pipeline, sized for one output.
pub(crate) struct BlurTextures {
    size: Size<i32, Physical>,
    levels: u32,
    /// Full-resolution snapshot of the world rendered by the frame code.
    accum: GlesTexture,
    /// Downsample chain; `chain[i]` is at `size / 2^(i+1)`.
    chain: Vec<GlesTexture>,
    /// Full-resolution blurred output.
    result: GlesTexture,
}

impl BlurTextures {
    pub(crate) fn accum(&self) -> &GlesTexture {
        &self.accum
    }

    pub(crate) fn result(&self) -> &GlesTexture {
        &self.result
    }
}

fn level_size(size: Size<i32, Physical>, level: u32) -> Size<i32, Physical> {
    let shift = level + 1;
    ((size.w >> shift).max(1), (size.h >> shift).max(1)).into()
}

fn create_texture(
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
) -> Result<GlesTexture, GlesError> {
    <GlesRenderer as Offscreen<GlesTexture>>::create_buffer(
        renderer,
        Fourcc::Argb8888,
        (size.w.max(1), size.h.max(1)).into(),
    )
}

pub(crate) fn ensure_scratch_texture(
    renderer: &mut GlesRenderer,
    existing: &mut Option<GlesTexture>,
    size: Size<i32, Physical>,
) -> Result<(), GlesError> {
    let size: Size<i32, Physical> = (size.w.max(1), size.h.max(1)).into();
    if let Some(texture) = existing.as_ref() {
        let texture_size: Size<i32, Buffer> = texture.size();
        if texture_size.w >= size.w && texture_size.h >= size.h {
            return Ok(());
        }
    }
    *existing = Some(create_texture(renderer, size)?);
    Ok(())
}

/// (Re)create the blur texture pool when missing or when the output size /
/// downsample depth changed. Returns the pool ready for use, or an error if a
/// texture could not be allocated.
pub(crate) fn ensure_blur_textures(
    renderer: &mut GlesRenderer,
    existing: &mut Option<BlurTextures>,
    size: Size<i32, Physical>,
    levels: u32,
) -> Result<(), GlesError> {
    let levels = levels.clamp(1, 5);
    if let Some(tex) = existing
        && tex.size == size
        && tex.levels == levels
    {
        return Ok(());
    }

    let accum = create_texture(renderer, size)?;
    let result = create_texture(renderer, size)?;
    let mut chain = Vec::with_capacity(levels as usize);
    for level in 0..levels {
        chain.push(create_texture(renderer, level_size(size, level))?);
    }

    *existing = Some(BlurTextures {
        size,
        levels,
        accum,
        chain,
        result,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn blur_pass(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    target_size: Size<i32, Physical>,
    source: &GlesTexture,
    source_size: Size<i32, Physical>,
    program: &GlesTexProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let mut bound = renderer.bind(target)?;
    let damage = Rectangle::<i32, Physical>::from_size(target_size);
    let mut frame = renderer.render(&mut bound, target_size, Transform::Normal)?;
    frame.clear(Color32F::TRANSPARENT, &[damage])?;

    let src = Rectangle::<f64, Buffer>::new(
        (0.0, 0.0).into(),
        (source_size.w as f64, source_size.h as f64).into(),
    );
    let dst = Rectangle::<i32, Physical>::from_size(target_size);
    let halfpixel = (
        0.5 / source_size.w.max(1) as f32,
        0.5 / source_size.h.max(1) as f32,
    );
    let uniforms = [
        Uniform::new("halfpixel", halfpixel),
        Uniform::new("offset", offset),
    ];
    frame.render_texture_from_to(
        source,
        src,
        dst,
        &[damage],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &uniforms,
    )?;
    let _ = frame.finish()?;
    Ok(())
}

/// Run the down/up Kawase chain, leaving the blurred world in `tex.result`.
///
/// Assumes the frame code has already rendered the backdrop into `tex.accum`.
pub(crate) fn run_blur(
    renderer: &mut GlesRenderer,
    tex: &mut BlurTextures,
    down_program: &GlesTexProgram,
    up_program: &GlesTexProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let levels = tex.levels as usize;
    let size = tex.size;

    // Downsample: accum -> chain[0] -> chain[1] -> ...
    blur_pass(
        renderer,
        &mut tex.chain[0],
        level_size(size, 0),
        &tex.accum,
        size,
        down_program,
        offset,
    )?;
    for i in 1..levels {
        let (lo, hi) = tex.chain.split_at_mut(i);
        blur_pass(
            renderer,
            &mut hi[0],
            level_size(size, i as u32),
            &lo[i - 1],
            level_size(size, (i - 1) as u32),
            down_program,
            offset,
        )?;
    }

    // Upsample back: chain[n-1] -> chain[n-2] -> ... -> chain[0] -> result.
    for i in (1..levels).rev() {
        let (lo, hi) = tex.chain.split_at_mut(i);
        blur_pass(
            renderer,
            &mut lo[i - 1],
            level_size(size, (i - 1) as u32),
            &hi[0],
            level_size(size, i as u32),
            up_program,
            offset,
        )?;
    }
    blur_pass(
        renderer,
        &mut tex.result,
        size,
        &tex.chain[0],
        level_size(size, 0),
        up_program,
        offset,
    )?;
    Ok(())
}

/// Map the configured blur radius to a Kawase sample-offset multiplier.
pub(crate) fn blur_offset(radius: f32) -> f32 {
    (radius / 16.0).clamp(0.6, 3.0)
}

/// Capture the current output framebuffer, blur it, and composite a patch at `dst`.
///
/// This is the same z-ordered backdrop model used by Niri's framebuffer effects:
/// call it immediately before drawing the translucent/blurred surface so the
/// captured pixels are exactly what is currently behind that surface.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_current_framebuffer_blur_patch(
    frame: &mut GlesFrame<'_, '_>,
    tex: &mut BlurTextures,
    down_program: &GlesTexProgram,
    up_program: &GlesTexProgram,
    composite_program: &GlesTexProgram,
    dst: Rectangle<i32, Physical>,
    corner_radius: f32,
    saturation: f32,
    noise: f32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
    offset: f32,
) -> Result<(), GlesError> {
    if dst.size.w <= 0 || dst.size.h <= 0 || alpha <= 0.0 || dst.intersection(damage).is_none() {
        return Ok(());
    }

    capture_current_framebuffer_and_blur(frame, tex, down_program, up_program, offset)?;

    composite_blur_patch(
        frame,
        tex.result(),
        composite_program,
        dst,
        corner_radius,
        saturation,
        noise,
        alpha,
        damage,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_current_framebuffer_blur_patch_masked(
    frame: &mut GlesFrame<'_, '_>,
    tex: &mut BlurTextures,
    down_program: &GlesTexProgram,
    up_program: &GlesTexProgram,
    composite_program: &GlesTexProgram,
    dst: Rectangle<i32, Physical>,
    mask: &GlesTexture,
    saturation: f32,
    noise: f32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
    offset: f32,
) -> Result<(), GlesError> {
    if dst.size.w <= 0 || dst.size.h <= 0 || alpha <= 0.0 || dst.intersection(damage).is_none() {
        return Ok(());
    }

    capture_current_framebuffer_and_blur(frame, tex, down_program, up_program, offset)?;

    composite_blur_patch_masked(
        frame,
        tex.result(),
        mask,
        composite_program,
        dst,
        saturation,
        noise,
        alpha,
        damage,
    )
}

fn capture_current_framebuffer_and_blur(
    frame: &mut GlesFrame<'_, '_>,
    tex: &mut BlurTextures,
    down_program: &GlesTexProgram,
    up_program: &GlesTexProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let size = tex.size;
    let accum = tex.accum();
    frame.with_context(|gl| unsafe {
        while gl.GetError() != ffi::NO_ERROR {}

        let mut current_fbo = 0i32;
        gl.GetIntegerv(ffi::DRAW_FRAMEBUFFER_BINDING, &mut current_fbo as *mut _);
        gl.Disable(ffi::SCISSOR_TEST);

        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo as *mut _);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
        gl.FramebufferTexture2D(
            ffi::DRAW_FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            accum.tex_id(),
            0,
        );
        gl.BlitFramebuffer(
            0,
            0,
            size.w,
            size.h,
            0,
            0,
            size.w,
            size.h,
            ffi::COLOR_BUFFER_BIT,
            ffi::NEAREST,
        );
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, current_fbo as u32);
        gl.Enable(ffi::SCISSOR_TEST);
        gl.DeleteFramebuffers(1, &mut fbo as *mut _);

        if gl.GetError() == ffi::NO_ERROR {
            Ok(())
        } else {
            Err(GlesError::BlitError)
        }
    })??;

    {
        let mut guard = frame.renderer();
        run_blur(guard.as_mut(), tex, down_program, up_program, offset)?;
    }

    Ok(())
}

/// Composite a blurred patch from `result` beneath a surface at `dst`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn composite_blur_patch(
    frame: &mut GlesFrame<'_, '_>,
    result: &GlesTexture,
    program: &GlesTexProgram,
    dst: Rectangle<i32, Physical>,
    corner_radius: f32,
    saturation: f32,
    noise: f32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), GlesError> {
    if dst.size.w <= 0 || dst.size.h <= 0 || alpha <= 0.0 {
        return Ok(());
    }
    let Some(visible) = dst.intersection(damage) else {
        return Ok(());
    };
    let local_damage = Rectangle::<i32, Physical>::new(
        (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
        visible.size,
    );

    // `result` is full output resolution, so the screen rect maps 1:1 into it.
    let src = Rectangle::<f64, Buffer>::new(
        (dst.loc.x as f64, dst.loc.y as f64).into(),
        (dst.size.w as f64, dst.size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("rect_size", (dst.size.w as f32, dst.size.h as f32)),
        Uniform::new(
            "patch_origin_uv",
            (
                dst.loc.x as f32 / result.size().w.max(1) as f32,
                dst.loc.y as f32 / result.size().h.max(1) as f32,
            ),
        ),
        Uniform::new(
            "patch_size_uv",
            (
                dst.size.w as f32 / result.size().w.max(1) as f32,
                dst.size.h as f32 / result.size().h.max(1) as f32,
            ),
        ),
        Uniform::new("corner_radius", corner_radius.max(0.0)),
        Uniform::new("saturation", saturation.clamp(0.0, 4.0)),
        Uniform::new("noise", noise.clamp(0.0, 1.0)),
    ];
    frame.render_texture_from_to(
        result,
        src,
        dst,
        &[local_damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn composite_blur_patch_masked(
    frame: &mut GlesFrame<'_, '_>,
    result: &GlesTexture,
    mask: &GlesTexture,
    program: &GlesTexProgram,
    dst: Rectangle<i32, Physical>,
    saturation: f32,
    noise: f32,
    alpha: f32,
    damage: Rectangle<i32, Physical>,
) -> Result<(), GlesError> {
    if dst.size.w <= 0 || dst.size.h <= 0 || alpha <= 0.0 {
        return Ok(());
    }
    let Some(visible) = dst.intersection(damage) else {
        return Ok(());
    };
    let local_damage = Rectangle::<i32, Physical>::new(
        (visible.loc.x - dst.loc.x, visible.loc.y - dst.loc.y).into(),
        visible.size,
    );

    frame.with_context(|gl| unsafe {
        gl.ActiveTexture(ffi::TEXTURE1);
        gl.BindTexture(ffi::TEXTURE_2D, mask.tex_id());
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MIN_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_MAG_FILTER, ffi::LINEAR as i32);
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_S,
            ffi::CLAMP_TO_EDGE as i32,
        );
        gl.TexParameteri(
            ffi::TEXTURE_2D,
            ffi::TEXTURE_WRAP_T,
            ffi::CLAMP_TO_EDGE as i32,
        );
        gl.ActiveTexture(ffi::TEXTURE0);
        Ok::<(), GlesError>(())
    })??;

    let src = Rectangle::<f64, Buffer>::new(
        (dst.loc.x as f64, dst.loc.y as f64).into(),
        (dst.size.w as f64, dst.size.h as f64).into(),
    );
    let uniforms = [
        Uniform::new("mask_tex", 1i32),
        Uniform::new(
            "patch_origin_uv",
            (
                dst.loc.x as f32 / result.size().w.max(1) as f32,
                dst.loc.y as f32 / result.size().h.max(1) as f32,
            ),
        ),
        Uniform::new(
            "patch_size_uv",
            (
                dst.size.w as f32 / result.size().w.max(1) as f32,
                dst.size.h as f32 / result.size().h.max(1) as f32,
            ),
        ),
        Uniform::new(
            "mask_uv_scale",
            (
                dst.size.w as f32 / mask.size().w.max(1) as f32,
                dst.size.h as f32 / mask.size().h.max(1) as f32,
            ),
        ),
        Uniform::new("saturation", saturation.clamp(0.0, 4.0)),
        Uniform::new("noise", noise.clamp(0.0, 1.0)),
    ];
    frame.render_texture_from_to(
        result,
        src,
        dst,
        &[local_damage],
        &[],
        Transform::Normal,
        alpha.clamp(0.0, 1.0),
        Some(program),
        &uniforms,
    )
}
