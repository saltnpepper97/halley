use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use drm::Device as BasicDevice;
use drm::control::{self as drm_control, Device as ControlDevice, ModeTypeFlags};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("outputs") => {
            if let Err(err) = print_outputs() {
                eprintln!("halleyctl outputs failed: {err}");
                std::process::exit(1);
            }
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!("halleyctl");
    println!();
    println!("Usage:");
    println!("  halleyctl outputs");
    println!();
    println!("Commands:");
    println!("  outputs   Print current output information from /sys/class/drm");
}

fn print_outputs() -> io::Result<()> {
    match print_outputs_via_drm() {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!("halleyctl: DRM ioctl path unavailable ({err}); falling back to sysfs");
            print_outputs_via_sysfs()
        }
    }
}

fn print_outputs_via_drm() -> io::Result<()> {
    let mut cards: Vec<PathBuf> = fs::read_dir("/dev/dri")?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"))
        })
        .collect();
    cards.sort();

    if cards.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no /dev/dri/card* devices found",
        ));
    }

    for card_path in cards {
        let card_name = card_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("card?");
        let card = Card::open(card_path.as_path())?;
        let resources = card.resource_handles()?;

        for conn in resources.connectors() {
            let info = card.get_connector(*conn, true)?;
            let full_name = format!(
                "{}-{}-{}",
                card_name,
                info.interface().as_str(),
                info.interface_id()
            );
            let status = match info.state() {
                drm_control::connector::State::Connected => "connected",
                drm_control::connector::State::Disconnected => "disconnected",
                drm_control::connector::State::Unknown => "unknown",
            };
            let current = current_mode(&card, &info);

            println!("{full_name}");
            println!("  status: {status}");
            println!(
                "  enabled: {}",
                if current.is_some() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if let Some(ref current_mode) = current {
                println!("  current_mode: {current_mode}");
            }
            if info.modes().is_empty() {
                println!("  modes: (none)");
            } else {
                println!("  modes:");
                for mode in info.modes() {
                    let mode_text = format_mode(mode);
                    let preferred = mode.mode_type().contains(ModeTypeFlags::PREFERRED);
                    let current_match = current.as_deref() == Some(mode_text.as_str());
                    let marker = if current_match {
                        "*"
                    } else if preferred {
                        "+"
                    } else {
                        "-"
                    };
                    println!("    {marker} {mode_text}");
                }
            }
        }
    }

    Ok(())
}

fn current_mode(card: &Card, info: &drm_control::connector::Info) -> Option<String> {
    let encoder = info
        .current_encoder()
        .or_else(|| info.encoders().first().copied())?;
    let encoder_info = card.get_encoder(encoder).ok()?;
    let crtc = encoder_info.crtc()?;
    let crtc_info = card.get_crtc(crtc).ok()?;
    crtc_info.mode().map(|m| format_mode(&m))
}

fn format_mode(mode: &drm_control::Mode) -> String {
    let (w, h) = mode.size();
    let hz = mode.vrefresh() as f64;
    format!("{w}x{h} @ {hz:.2}Hz")
}

fn print_outputs_via_sysfs() -> io::Result<()> {
    let drm_root = Path::new("/sys/class/drm");
    let mut entries: Vec<PathBuf> = fs::read_dir(drm_root)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_connector_name)
        })
        .collect();

    entries.sort();

    if entries.is_empty() {
        println!(
            "No DRM connector entries found under {}",
            drm_root.display()
        );
        return Ok(());
    }

    for entry in entries {
        let name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>");
        let status = read_trimmed(entry.join("status")).unwrap_or_else(|| "unknown".to_string());
        let enabled = read_trimmed(entry.join("enabled"));
        let current_mode = read_trimmed(entry.join("mode"));
        let modes = read_modes(entry.join("modes"));

        println!("{name}");
        println!("  status: {status}");
        if let Some(enabled) = enabled {
            println!("  enabled: {enabled}");
        }
        if let Some(current_mode) = &current_mode {
            println!("  current_mode: {current_mode}");
        }
        if modes.is_empty() {
            println!("  modes: (none)");
        } else {
            println!("  modes: (resolution-only; refresh unavailable via sysfs)");
            for mode in modes {
                let marker = if current_mode.as_deref() == Some(mode.as_str()) {
                    "*"
                } else {
                    "-"
                };
                println!("    {marker} {mode}");
            }
        }
    }

    Ok(())
}

fn is_connector_name(name: &str) -> bool {
    name.starts_with("card") && name.contains('-')
}

fn read_trimmed(path: PathBuf) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn read_modes(path: PathBuf) -> Vec<String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

struct Card(fs::File);

impl Card {
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .or_else(|_| OpenOptions::new().read(true).open(path))?;
        Ok(Self(file))
    }
}

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}
