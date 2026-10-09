import type { InputEvent, MouseButton } from "./api";

const BUTTONS: Record<number, MouseButton | undefined> = { 0: "left", 1: "middle", 2: "right" };

/** Pasangan tombol yang ditukar saat opsi "tukar Cmd dan Ctrl" aktif. */
const SWAPPED: Record<string, string | undefined> = {
  MetaLeft: "ControlLeft",
  MetaRight: "ControlRight",
  ControlLeft: "MetaLeft",
  ControlRight: "MetaRight",
};

/** Tinggi satu baris teks, untuk roda mouse yang melaporkan scroll per baris. */
const LINE_PIXELS = 16;

export interface InputOptions {
  /** Dibaca tiap kali tombol ditekan, jadi bisa berubah selama sesi. */
  swapModifiers: () => boolean;
}

/**
 * Menangkap mouse dan keyboard di atas `canvas` dan meneruskannya ke `send`,
 * paling banyak satu kiriman per frame animasi. Mengembalikan fungsi untuk
 * melepas semua listener.
 */
export function attachInput(
  canvas: HTMLCanvasElement,
  send: (events: InputEvent[]) => void,
  options: InputOptions,
): () => void {
  const queue: InputEvent[] = [];
  let pendingMove: { x: number; y: number } | null = null;
  let pendingScroll: { dx: number; dy: number } | null = null;
  let scheduled = false;
  /** `code` asli → `code` yang dikirim, untuk tombol yang sedang ditekan. */
  const held = new Map<string, string>();

  const position = (event: MouseEvent) => {
    const rect = canvas.getBoundingClientRect();
    return {
      x: Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width)),
      y: Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height)),
    };
  };

  /** Memindahkan gerakan dan scroll yang tertunda ke antrean, menjaga urutan. */
  const settle = () => {
    if (pendingMove) queue.push({ t: "move", ...pendingMove });
    if (pendingScroll) queue.push({ t: "scroll", ...pendingScroll });
    pendingMove = null;
    pendingScroll = null;
  };

  const flush = () => {
    scheduled = false;
    settle();
    if (queue.length > 0) send(queue.splice(0));
  };

  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(flush);
  };

  const push = (event: InputEvent) => {
    settle();
    queue.push(event);
    schedule();
  };

  const onPointerMove = (event: PointerEvent) => {
    pendingMove = position(event);
    schedule();
  };

  const onPointerButton = (down: boolean) => (event: PointerEvent) => {
    const button = BUTTONS[event.button];
    if (button === undefined) return;
    event.preventDefault();
    // Dengan pointer capture, gerakan dan pelepasan di luar kanvas tetap diterima.
    if (down) canvas.setPointerCapture(event.pointerId);
    pendingMove = null;
    push({ t: "button", button, down, ...position(event) });
  };

  const onWheel = (event: WheelEvent) => {
    event.preventDefault();
    const unit =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? LINE_PIXELS
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? canvas.clientHeight
          : 1;
    pendingScroll = {
      dx: (pendingScroll?.dx ?? 0) + event.deltaX * unit,
      dy: (pendingScroll?.dy ?? 0) + event.deltaY * unit,
    };
    schedule();
  };

  const onKey = (down: boolean) => (event: KeyboardEvent) => {
    if (event.code === "") return;
    // Mencegah tombol seperti Tab atau spasi bekerja di jendela ini.
    event.preventDefault();
    if (down) {
      const code = options.swapModifiers() ? (SWAPPED[event.code] ?? event.code) : event.code;
      held.set(event.code, code);
      push({ t: "key", code, down: true });
    } else {
      // Dilepas dengan kode yang sama seperti saat ditekan, walau opsi tukar berubah di tengah.
      const code = held.get(event.code);
      if (code === undefined) return;
      held.delete(event.code);
      push({ t: "key", code, down: false });
    }
  };

  /** Saat jendela kehilangan fokus, `keyup` tidak akan datang lagi. */
  const releaseAll = () => {
    for (const code of held.values()) queue.push({ t: "key", code, down: false });
    held.clear();
    flush();
  };

  const onContextMenu = (event: Event) => event.preventDefault();
  const onPointerDown = onPointerButton(true);
  const onPointerUp = onPointerButton(false);
  const onKeyDown = onKey(true);
  const onKeyUp = onKey(false);

  canvas.addEventListener("pointermove", onPointerMove);
  canvas.addEventListener("pointerdown", onPointerDown);
  canvas.addEventListener("pointerup", onPointerUp);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("contextmenu", onContextMenu);
  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("keyup", onKeyUp);
  window.addEventListener("blur", releaseAll);

  return () => {
    releaseAll();
    canvas.removeEventListener("pointermove", onPointerMove);
    canvas.removeEventListener("pointerdown", onPointerDown);
    canvas.removeEventListener("pointerup", onPointerUp);
    canvas.removeEventListener("wheel", onWheel);
    canvas.removeEventListener("contextmenu", onContextMenu);
    window.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("keyup", onKeyUp);
    window.removeEventListener("blur", releaseAll);
  };
}
