import { Channel, invoke } from "@tauri-apps/api/core";
import { codecFromSps, H264Decoder } from "./decoder";
import { parseFrame } from "./frame";

/** Ringkasan statistik (milidetik) dari sekumpulan sampel. */
function summarize(samples: number[]) {
  if (samples.length === 0) return null;
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (p: number) => Number(sorted[Math.round((sorted.length - 1) * p)].toFixed(2));
  const mean = samples.reduce((sum, value) => sum + value, 0) / samples.length;
  return { mean: Number(mean.toFixed(2)), p50: at(0.5), p95: at(0.95), max: at(1) };
}

/** Rentang kecerahan di pojok kiri atas kanvas; gambar kosong memberi rentang nol. */
function lumaRange(canvas: HTMLCanvasElement) {
  const context = canvas.getContext("2d");
  if (context === null || canvas.width < 64 || canvas.height < 64) return null;
  const { data } = context.getImageData(0, 0, 64, 64);
  let min = 255;
  let max = 0;
  for (let i = 0; i < data.length; i += 4) {
    const luma = (data[i] + data[i + 1] + data[i + 2]) / 3;
    min = Math.min(min, luma);
    max = Math.max(max, luma);
  }
  return { min: Math.round(min), max: Math.round(max) };
}

/**
 * Uji mandiri video: memutar stream sintetis dari Rust lewat pipeline decode
 * yang sama dengan sesi sungguhan, lalu melaporkan hasilnya ke Rust.
 */
export async function runSelftest(canvas: HTMLCanvasElement): Promise<void> {
  const context = canvas.getContext("2d");
  const errors: string[] = [];
  const ipcMs: number[] = [];
  const decodeMs: number[] = [];
  const submittedAt = new Map<number, number>();
  let received = 0;
  let decoded = 0;
  let rejected = 0;
  let codec = "";
  let size = "";

  const report = async (extra: Record<string, unknown> = {}) => {
    await invoke("selftest_report", {
      report: JSON.stringify({
        userAgent: navigator.userAgent,
        hasVideoDecoder: typeof VideoDecoder !== "undefined",
        codec,
        size,
        received,
        decoded,
        rejected,
        ipcMs: summarize(ipcMs),
        decodeMs: summarize(decodeMs),
        luma: lumaRange(canvas),
        errors: errors.slice(0, 5),
        ...extra,
      }),
    });
  };

  if (typeof VideoDecoder === "undefined" || context === null) {
    await report();
    return;
  }

  const decoder = new H264Decoder(
    (frame) => {
      const started = submittedAt.get(frame.timestamp);
      if (started !== undefined) {
        decodeMs.push(performance.now() - started);
        submittedAt.delete(frame.timestamp);
      }
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
        size = `${frame.displayWidth}x${frame.displayHeight}`;
      }
      context.drawImage(frame, 0, 0);
      frame.close();
      decoded += 1;
    },
    (message) => errors.push(message),
  );

  const channel = new Channel<ArrayBuffer>();
  const finished = new Promise<void>((resolve) => {
    channel.onmessage = (buffer) => {
      // Pesan kosong menandai akhir stream uji.
      if (buffer.byteLength === 0) {
        resolve();
        return;
      }
      const frame = parseFrame(buffer);
      received += 1;
      ipcMs.push(performance.timeOrigin + performance.now() - frame.sentEpochUs / 1000);
      if (frame.keyframe && codec === "") codec = codecFromSps(frame.data) ?? "tanpa SPS";
      submittedAt.set(frame.timestampUs, performance.now());
      if (!decoder.decode(frame)) rejected += 1;
    };
  });

  try {
    await invoke("selftest_stream", { onFrame: channel });
    await finished;
    await decoder.flush();
  } catch (error) {
    errors.push(String(error));
  }
  await report();
}
