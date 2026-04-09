pub mod capture;
pub mod screencopy;

pub use capture::{
    CaptureCrop, crop_image, default_output_path, default_output_path_in, save_cropped_png,
    temp_output_path,
};
pub use screencopy::capture_desktop_to_temp_file;
