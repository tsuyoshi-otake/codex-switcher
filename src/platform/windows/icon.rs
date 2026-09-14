//! Tray icon drawn at runtime (no resource files): a filled circle with a "C" ring.

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIcon, HICON};

const SIZE: usize = 32;

fn pixels(busy: bool) -> Vec<u8> {
    let (r, g, b) = if busy { (0xF5u8, 0x9Eu8, 0x0Bu8) } else { (0x4F, 0x46, 0xE5) };
    let mut bgra = vec![0u8; SIZE * SIZE * 4];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (dx, dy) = (x as f32 - center, y as f32 - center);
            let d = (dx * dx + dy * dy).sqrt();
            let coverage = (15.5 - d).clamp(0.0, 1.0);
            if coverage == 0.0 {
                continue;
            }
            let angle = dy.atan2(dx);
            let in_ring = (6.5..=11.0).contains(&d) && angle.abs() > 0.75;
            let (pr, pg, pb) = if in_ring { (0xFF, 0xFF, 0xFF) } else { (r, g, b) };
            let i = (y * SIZE + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[pb, pg, pr, (coverage * 255.0) as u8]);
        }
    }
    bgra
}

pub fn create(busy: bool) -> HICON {
    let xor = pixels(busy);
    let and = vec![0u8; SIZE.div_ceil(16) * 2 * SIZE];
    // SAFETY: buffers match the declared 32x32 / 32bpp / 1bpp-mask dimensions.
    unsafe {
        CreateIcon(GetModuleHandleW(std::ptr::null()), SIZE as i32, SIZE as i32, 1, 32, and.as_ptr(), xor.as_ptr())
    }
}
