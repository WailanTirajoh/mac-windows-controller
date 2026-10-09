//! Sumber frame sintetis untuk test, benchmark, dan uji mandiri video.

use super::I420Frame;

/// Nilai acak tetap untuk sel "huruf" di posisi tertentu.
fn glyph(cell_x: usize, cell_y: usize) -> u8 {
    let mut hash = (cell_x as u32).wrapping_mul(0x9E37_79B1) ^ (cell_y as u32).wrapping_mul(0x85EB_CA77);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2C1B_3C6D);
    if (hash >> 13) & 3 == 0 {
        30
    } else {
        235
    }
}

/// Frame ke-`index` dari "dokumen yang di-scroll": pola kontras tinggi mirip
/// teks yang bergeser 8 piksel per frame di 60% kiri layar, sementara sisanya
/// diam. Ini kasus berat yang umum untuk encoder layar.
pub fn scrolling_document(width: u32, height: u32, index: u32) -> I420Frame {
    let mut frame = I420Frame::new(width, height, u64::from(index) * 16_667);
    let w = width as usize;
    let scroll = index as usize * 8;
    let content = w * 6 / 10;
    for (row, line) in frame.y.chunks_exact_mut(w).enumerate() {
        for (col, pixel) in line.iter_mut().enumerate() {
            let y = if col < content { row + scroll } else { row };
            // Baris teks setinggi 12 px dengan jarak antarbaris 6 px.
            *pixel = if y % 18 < 12 { glyph(col / 3, y / 3) } else { 235 };
        }
    }
    frame
}
