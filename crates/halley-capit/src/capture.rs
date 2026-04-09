use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{DynamicImage, GenericImageView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCrop {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn crop_image(img: DynamicImage, crop: CaptureCrop) -> Result<DynamicImage, String> {
    let (iw, ih) = img.dimensions();
    let x0 = crop.x.max(0) as u32;
    let y0 = crop.y.max(0) as u32;
    let x1 = (crop.x.max(0) as u32)
        .saturating_add(crop.w.max(0) as u32)
        .min(iw);
    let y1 = (crop.y.max(0) as u32)
        .saturating_add(crop.h.max(0) as u32)
        .min(ih);
    let cw = x1.saturating_sub(x0);
    let ch = y1.saturating_sub(y0);
    if cw == 0 || ch == 0 {
        return Err(format!(
            "crop rect empty after clamping: ({},{}) {}x{} within {}x{}",
            crop.x, crop.y, crop.w, crop.h, iw, ih
        ));
    }
    Ok(img.crop_imm(x0, y0, cw, ch))
}

pub fn save_cropped_png(src_path: &Path, out_path: &Path, crop: CaptureCrop) -> Result<(), String> {
    let img = image::open(src_path).map_err(|e| format!("open screenshot: {e}"))?;
    let cropped = crop_image(img, crop)?;
    ensure_parent_dir(out_path)?;
    cropped
        .save(out_path)
        .map_err(|e| format!("save cropped screenshot: {e}"))
}

pub fn default_output_path(stem: &str) -> PathBuf {
    default_output_path_in(
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Pictures")
            .join("Screenshots"),
        stem,
    )
}

pub fn default_output_path_in(dir: PathBuf, stem: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.join(format!("{stem}-{nanos}.png"))
}

pub fn temp_output_path(final_out_path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut p = final_out_path.to_path_buf();
    let stem = final_out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("capit");
    p.set_file_name(format!("{stem}.halley_capit_tmp_{nanos}.png"));
    p
}

fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir {parent:?}: {e}"))?;
    }
    Ok(())
}
