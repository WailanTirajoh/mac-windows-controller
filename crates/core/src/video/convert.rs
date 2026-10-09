//! Konversi format piksel ke [`I420Frame`].

use anyhow::Context;
use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use yuv::{BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix};

use super::I420Frame;

const BGRA_BYTES: usize = 4;

/// Mengubah gambar BGRA menjadi I420 (BT.709 rentang video), diperkecil dulu
/// kalau ukuran tujuannya lebih kecil. Buffer kerjanya dipakai ulang antar
/// frame.
#[derive(Default)]
pub struct BgraConverter {
    resizer: Resizer,
    /// Gambar sumber tanpa padding baris, untuk diperkecil.
    packed: Vec<u8>,
    scaled: Vec<u8>,
}

impl BgraConverter {
    /// `stride` adalah jumlah byte per baris di `bgra`, boleh lebih besar
    /// dari `4 * lebar`. `target` harus genap dan tidak lebih besar dari `size`.
    pub fn convert(
        &mut self,
        bgra: &[u8],
        stride: usize,
        size: (u32, u32),
        target: (u32, u32),
        timestamp_us: u64,
    ) -> anyhow::Result<I420Frame> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        anyhow::ensure!(width > 0 && height > 0, "gambar kosong");
        anyhow::ensure!(
            stride >= width * BGRA_BYTES && bgra.len() >= stride * (height - 1) + width * BGRA_BYTES,
            "buffer gambar lebih kecil dari ukurannya"
        );
        anyhow::ensure!(target.0 <= size.0 && target.1 <= size.1, "ukuran tujuan lebih besar dari sumber");

        // Selisih satu piksel (sumber berukuran ganjil) cukup dipotong, tidak perlu diskala.
        let needs_scaling = size.0 - target.0 > 1 || size.1 - target.1 > 1;
        let (pixels, pixel_stride) = if needs_scaling {
            self.scale(bgra, stride, size, target)?;
            (self.scaled.as_slice(), target.0 as usize * BGRA_BYTES)
        } else {
            (bgra, stride)
        };

        let mut frame = I420Frame::new(target.0, target.1, timestamp_us);
        let mut planes = YuvPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(&mut frame.y),
            y_stride: target.0,
            u_plane: BufferStoreMut::Borrowed(&mut frame.u),
            u_stride: target.0 / 2,
            v_plane: BufferStoreMut::Borrowed(&mut frame.v),
            v_stride: target.0 / 2,
            width: target.0,
            height: target.1,
        };
        yuv::bgra_to_yuv420(
            &mut planes,
            pixels,
            pixel_stride as u32,
            YuvRange::Limited,
            YuvStandardMatrix::Bt709,
            YuvConversionMode::default(),
        )
        .map_err(|error| anyhow::anyhow!("konversi warna gagal: {error:?}"))?;
        drop(planes);
        Ok(frame)
    }

    /// Memperkecil `bgra` ke `target`, hasilnya di `self.scaled`.
    fn scale(&mut self, bgra: &[u8], stride: usize, size: (u32, u32), target: (u32, u32)) -> anyhow::Result<()> {
        let row_bytes = size.0 as usize * BGRA_BYTES;
        let source = if stride == row_bytes {
            &bgra[..row_bytes * size.1 as usize]
        } else {
            self.packed.clear();
            for row in bgra.chunks(stride).take(size.1 as usize) {
                self.packed.extend_from_slice(&row[..row_bytes]);
            }
            self.packed.as_slice()
        };

        self.scaled.resize(target.0 as usize * target.1 as usize * BGRA_BYTES, 0);
        let source = ImageRef::new(size.0, size.1, source, PixelType::U8x4).context("gambar sumber tidak valid")?;
        let mut scaled = Image::from_slice_u8(target.0, target.1, &mut self.scaled, PixelType::U8x4)
            .context("buffer hasil skala tidak valid")?;
        // Alpha dari capture layar tidak bermakna, jadi tidak ikut dihitung.
        let options = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            .use_alpha(false);
        self.resizer
            .resize(&source, &mut scaled, &options)
            .context("gagal memperkecil gambar")?;
        Ok(())
    }
}

/// Menyalin gambar NV12 (plane Y + plane CbCr berselang) ke `out`.
///
/// `y_stride` dan `uv_stride` boleh lebih besar dari lebar gambar (padding
/// baris). Ukuran `out` menentukan jumlah piksel yang disalin.
pub fn nv12_to_i420(y: &[u8], y_stride: usize, uv: &[u8], uv_stride: usize, out: &mut I420Frame) {
    let width = out.width as usize;
    let height = out.height as usize;
    let chroma_w = width / 2;
    let chroma_h = height / 2;

    for (dst, src) in out.y.chunks_exact_mut(width).zip(y.chunks(y_stride)).take(height) {
        dst.copy_from_slice(&src[..width]);
    }

    let rows = out
        .u
        .chunks_exact_mut(chroma_w)
        .zip(out.v.chunks_exact_mut(chroma_w))
        .zip(uv.chunks(uv_stride))
        .take(chroma_h);
    for ((dst_u, dst_v), src) in rows {
        for ((u, v), pair) in dst_u.iter_mut().zip(dst_v.iter_mut()).zip(src.chunks_exact(2)) {
            *u = pair[0];
            *v = pair[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_planes_are_split_and_padding_is_dropped() {
        // Gambar 4x2 dengan stride 6 (2 byte padding per baris).
        let y = [1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99];
        let uv = [10, 20, 11, 21, 99, 99];
        let mut out = I420Frame::new(4, 2, 0);

        nv12_to_i420(&y, 6, &uv, 6, &mut out);

        assert_eq!(out.y, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(out.u, [10, 11]);
        assert_eq!(out.v, [20, 21]);
    }

    /// Gambar satu warna berukuran `width` x `height`, dengan `padding` byte
    /// sampah di akhir tiap baris.
    fn solid(bgra: [u8; 4], width: usize, height: usize, padding: usize) -> (Vec<u8>, usize) {
        let stride = width * 4 + padding;
        let mut image = Vec::with_capacity(stride * height);
        for _ in 0..height {
            for _ in 0..width {
                image.extend_from_slice(&bgra);
            }
            image.extend(std::iter::repeat_n(0xAB, padding));
        }
        (image, stride)
    }

    /// Memastikan semua nilai di `plane` berada dalam `tolerance` dari `expected`.
    fn assert_plane(plane: &[u8], expected: u8, tolerance: u8, name: &str) {
        for value in plane {
            assert!(value.abs_diff(expected) <= tolerance, "{name}: {value}, seharusnya sekitar {expected}");
        }
    }

    fn convert(bgra: [u8; 4], size: (u32, u32), padding: usize, target: (u32, u32)) -> I420Frame {
        let (image, stride) = solid(bgra, size.0 as usize, size.1 as usize, padding);
        BgraConverter::default().convert(&image, stride, size, target, 7).unwrap()
    }

    #[test]
    fn bgra_colors_map_to_bt709_video_range() {
        // Nilai acuan BT.709 rentang video: putih (235,128,128), hitam (16,128,128), merah (63,102,240).
        for (bgra, y, u, v) in [
            ([255, 255, 255, 255], 235, 128, 128),
            ([0, 0, 0, 255], 16, 128, 128),
            ([0, 0, 255, 255], 63, 102, 240),
        ] {
            let frame = convert(bgra, (8, 4), 0, (8, 4));
            assert_plane(&frame.y, y, 2, "Y");
            assert_plane(&frame.u, u, 2, "U");
            assert_plane(&frame.v, v, 2, "V");
            assert_eq!((frame.width, frame.height, frame.timestamp_us), (8, 4, 7));
        }
    }

    #[test]
    fn row_padding_does_not_leak_into_the_picture() {
        let frame = convert([0, 0, 0, 255], (8, 4), 12, (8, 4));
        assert_plane(&frame.y, 16, 2, "Y");
    }

    #[test]
    fn larger_picture_is_scaled_down_to_the_target() {
        for padding in [0, 20] {
            let frame = convert([255, 255, 255, 255], (64, 40), padding, (32, 20));
            assert_eq!((frame.width, frame.height), (32, 20));
            assert_eq!(frame.y.len(), 32 * 20);
            assert_plane(&frame.y, 235, 2, "Y");
            assert_plane(&frame.u, 128, 2, "U");
        }
    }

    #[test]
    fn odd_sized_source_is_cropped_to_even() {
        let frame = convert([255, 255, 255, 255], (9, 5), 0, (8, 4));
        assert_eq!((frame.width, frame.height), (8, 4));
        assert_plane(&frame.y, 235, 2, "Y");
    }

    #[test]
    fn undersized_buffer_is_rejected() {
        let (image, stride) = solid([0, 0, 0, 255], 8, 4, 0);
        let result = BgraConverter::default().convert(&image[..image.len() - 8], stride, (8, 4), (8, 4), 0);
        assert!(result.is_err());
    }
}
