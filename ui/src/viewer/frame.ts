/** Satu frame video dari proses Rust. Formatnya ada di `src-tauri/src/viewer_frame.rs`. */
export interface ViewerFrame {
  keyframe: boolean;
  width: number;
  height: number;
  /** Waktu capture di perangkat tujuan, mikrodetik. */
  timestampUs: number;
  /** Waktu jam dinding saat Rust mengirim frame ini ke webview, mikrodetik. */
  sentEpochUs: number;
  /** Satu access unit H.264 format Annex B. */
  data: Uint8Array;
}

const HEADER_BYTES = 28;
const FLAG_KEYFRAME = 1;

export function parseFrame(buffer: ArrayBuffer): ViewerFrame {
  const view = new DataView(buffer);
  return {
    keyframe: (view.getUint8(0) & FLAG_KEYFRAME) !== 0,
    width: view.getUint32(4, true),
    height: view.getUint32(8, true),
    timestampUs: Number(view.getBigUint64(12, true)),
    sentEpochUs: Number(view.getBigUint64(20, true)),
    data: new Uint8Array(buffer, HEADER_BYTES),
  };
}
