import type { ViewerFrame } from "./frame";

const NAL_TYPE_SPS = 7;

/**
 * Membaca string codec (`avc1.PPCCLL`) dari SPS di dalam access unit Annex B.
 * Mengembalikan `null` kalau tidak ada SPS (bukan keyframe).
 */
export function codecFromSps(data: Uint8Array): string | null {
  for (let i = 0; i + 6 < data.length; i++) {
    if (data[i] !== 0 || data[i + 1] !== 0 || data[i + 2] !== 1) continue;
    if ((data[i + 3] & 0x1f) !== NAL_TYPE_SPS) continue;
    const hex = (byte: number) => byte.toString(16).padStart(2, "0");
    return `avc1.${hex(data[i + 4])}${hex(data[i + 5])}${hex(data[i + 6])}`;
  }
  return null;
}

/**
 * Decoder H.264 di atas WebCodecs. Frame hasil decode diserahkan ke `onFrame`,
 * yang wajib memanggil `frame.close()`.
 */
export class H264Decoder {
  private decoder: VideoDecoder | null = null;
  private configured = "";
  private needsKeyframe = true;
  private lastTimestampUs = -1;

  constructor(
    private readonly onFrame: (frame: VideoFrame) => void,
    private readonly onError: (message: string) => void,
  ) {}

  /**
   * Mengirim satu frame ke decoder. Mengembalikan `false` kalau frame dibuang
   * karena decoder masih menunggu keyframe; pemanggil sebaiknya meminta
   * keyframe ke pengirim.
   */
  decode(frame: ViewerFrame): boolean {
    if (frame.keyframe) {
      const codec = codecFromSps(frame.data);
      if (codec === null) {
        this.onError("keyframe tanpa SPS");
        return false;
      }
      // Timestamp yang mundur berarti stream baru (sesi dimulai ulang). Decoder
      // lama bisa menahan frame seperti itu, jadi dibuat ulang di keyframe ini.
      if (frame.timestampUs <= this.lastTimestampUs) this.close();
      this.configure(codec, frame.width, frame.height);
      this.needsKeyframe = false;
    }
    if (this.needsKeyframe || this.decoder === null) return false;
    this.lastTimestampUs = frame.timestampUs;

    this.decoder.decode(
      new EncodedVideoChunk({
        type: frame.keyframe ? "key" : "delta",
        timestamp: frame.timestampUs,
        data: frame.data,
      }),
    );
    return true;
  }

  /** Jumlah frame yang sudah dikirim ke decoder tetapi belum selesai. */
  get queueSize(): number {
    return this.decoder?.decodeQueueSize ?? 0;
  }

  async flush(): Promise<void> {
    if (this.decoder?.state === "configured") await this.decoder.flush();
  }

  close(): void {
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
    this.needsKeyframe = true;
    this.lastTimestampUs = -1;
  }

  private configure(codec: string, width: number, height: number): void {
    const key = `${codec}:${width}x${height}`;
    if (this.decoder?.state === "configured" && key === this.configured) return;

    this.close();
    const decoder = new VideoDecoder({
      output: this.onFrame,
      error: (error) => {
        // Setelah error decoder tertutup; mulai lagi dari keyframe berikutnya.
        this.needsKeyframe = true;
        this.onError(error.message);
      },
    });
    // Tanpa `description`, bitstream dibaca sebagai Annex B.
    decoder.configure({ codec, codedWidth: width, codedHeight: height, optimizeForLatency: true });
    this.decoder = decoder;
    this.configured = key;
  }
}
