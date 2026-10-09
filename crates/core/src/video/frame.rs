/// Gambar YUV 4:2:0 planar (I420), BT.709 rentang video.
///
/// Tiap plane rapat (stride sama dengan lebar plane) dan lebar/tinggi selalu
/// genap.
#[derive(Clone)]
pub struct I420Frame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    /// Waktu capture dalam mikrodetik sejak capture dimulai.
    pub timestamp_us: u64,
}

impl I420Frame {
    /// Membuat frame hitam berukuran `width` x `height` (keduanya harus genap).
    pub fn new(width: u32, height: u32, timestamp_us: u64) -> Self {
        assert!(width % 2 == 0 && height % 2 == 0, "ukuran I420 harus genap");
        let luma = width as usize * height as usize;
        Self {
            width,
            height,
            y: vec![16; luma],
            u: vec![128; luma / 4],
            v: vec![128; luma / 4],
            timestamp_us,
        }
    }
}

/// Satu access unit H.264 dalam format Annex B.
#[derive(Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
}

/// Mengecilkan `(width, height)` agar sisi panjangnya tidak melebihi
/// `max_long_edge`, menjaga rasio. Hasilnya selalu genap dan tidak pernah
/// lebih besar dari ukuran asal.
pub fn fit_even(width: u32, height: u32, max_long_edge: u32) -> (u32, u32) {
    let long = width.max(height);
    let (w, h) = if long > max_long_edge && long > 0 {
        let scale = f64::from(max_long_edge) / f64::from(long);
        (
            (f64::from(width) * scale).round() as u32,
            (f64::from(height) * scale).round() as u32,
        )
    } else {
        (width, height)
    };
    ((w & !1).max(2), (h & !1).max(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_even_keeps_small_sizes() {
        assert_eq!(fit_even(1280, 720, 1920), (1280, 720));
    }

    #[test]
    fn fit_even_scales_down_and_keeps_ratio() {
        assert_eq!(fit_even(2560, 1664, 1920), (1920, 1248));
        assert_eq!(fit_even(1664, 2560, 1920), (1248, 1920));
    }

    #[test]
    fn fit_even_rounds_odd_sizes_down() {
        assert_eq!(fit_even(1367, 769, 4000), (1366, 768));
    }
}
