//! Konversi format piksel ke [`I420Frame`].

use super::I420Frame;

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
}
