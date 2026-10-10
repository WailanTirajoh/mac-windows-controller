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

const wait = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

/** Jumlah nilai piksel di satu baris kanvas; berubah kalau gambar bergerak. */
function rowChecksum(canvas: HTMLCanvasElement): number {
  const context = canvas.getContext("2d");
  if (context === null || canvas.width === 0) return 0;
  const { data } = context.getImageData(0, Math.floor(canvas.height / 2), Math.min(canvas.width, 256), 1);
  return data.reduce((sum, value) => sum + value, 0);
}

/**
 * Uji mandiri sesi: mengamati jendela viewer sungguhan yang terhubung ke node
 * tujuan bersumber layar sintetis. Memeriksa bahwa video tampil dan bergerak,
 * lalu mengirim event mouse dan keyboard buatan lewat kanvas.
 */
export async function runSessionSelftest(): Promise<void> {
  const errors: string[] = [];
  const canvas = await (async () => {
    for (let attempt = 0; attempt < 100; attempt++) {
      const found = document.querySelector("canvas");
      // Lebar bawaan kanvas kosong adalah 300; lebih dari itu berarti frame sudah digambar.
      if (found !== null && found.width > 300) return found;
      await wait(100);
    }
    return null;
  })();

  if (canvas === null) {
    errors.push("tidak ada frame yang tampil dalam 10 detik");
    await invoke("selftest_report", { report: JSON.stringify({ errors, page: document.body.innerText }) });
    return;
  }

  const before = rowChecksum(canvas);
  await wait(500);
  const moving = rowChecksum(canvas) !== before;

  // Event buatan lewat elemen yang sama dengan yang dipakai pengguna.
  const rect = canvas.getBoundingClientRect();
  const at = { clientX: rect.left + rect.width / 4, clientY: rect.top + rect.height / 2, bubbles: true };
  canvas.dispatchEvent(new PointerEvent("pointermove", at));
  await wait(50);
  const pressKey = (code: string) => {
    window.dispatchEvent(new KeyboardEvent("keydown", { code, bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keyup", { code, bubbles: true }));
  };
  pressKey("KeyA");
  await wait(400);
  const size = `${canvas.width}x${canvas.height}`;
  const luma = lumaRange(canvas);

  /** Memilih nilai di salah satu pilihan toolbar, seperti yang dilakukan pengguna. */
  const choose = (label: string, value: string) => {
    const select = document.querySelector<HTMLSelectElement>(`select[aria-label="${label}"]`);
    if (select === null) {
      errors.push(`pilihan "${label}" tidak ada`);
      return;
    }
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  };

  // Ganti kualitas memulai ulang sesi: video dan input harus tetap jalan.
  choose("Kualitas video", "low");
  await wait(1500);
  const beforeRestartCheck = rowChecksum(canvas);
  await wait(500);
  const movingAfterQuality = rowChecksum(canvas) !== beforeRestartCheck;
  pressKey("KeyB");
  await wait(400);

  // Pindah ke layar kedua, yang ukurannya berbeda.
  choose("Pilih layar", "2");
  for (let attempt = 0; attempt < 50 && canvas.width === 1280; attempt++) await wait(100);

  await invoke("selftest_report", {
    report: JSON.stringify({
      size,
      luma,
      moving,
      movingAfterQuality,
      sizeAfterDisplayChange: `${canvas.width}x${canvas.height}`,
      toolbar: document.querySelector("header")?.textContent ?? "",
      errors,
    }),
  });
}
