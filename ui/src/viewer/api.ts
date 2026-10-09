import { type Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface RemoteDisplay {
  id: number;
  /** Ukuran dalam piksel fisik. */
  width: number;
  height: number;
  isPrimary: boolean;
}

export interface SessionView {
  name: string;
  displays: RemoteDisplay[];
  displayId: number;
  /** `false` berarti sesi hanya untuk melihat. */
  inputEnabled: boolean;
}

export type MouseButton = "left" | "right" | "middle";

/** Event input untuk perangkat tujuan; posisi dinyatakan 0..1 relatif terhadap layar yang tampil. */
export type InputEvent =
  | { t: "move"; x: number; y: number }
  | { t: "button"; button: MouseButton; down: boolean; x: number; y: number }
  | { t: "scroll"; dx: number; dy: number }
  | { t: "key"; code: string; down: boolean };

export const viewerStart = (fingerprint: string, onFrame: Channel<ArrayBuffer>) =>
  invoke<SessionView>("viewer_start", { fingerprint, onFrame });
export const viewerInput = (fingerprint: string, events: InputEvent[]) =>
  invoke<void>("viewer_input", { fingerprint, events });
export const viewerSelectDisplay = (fingerprint: string, displayId: number) =>
  invoke<void>("viewer_select_display", { fingerprint, displayId });
export const viewerRequestKeyframe = (fingerprint: string) => invoke<void>("viewer_request_keyframe", { fingerprint });

export const onDisplayChanged = (handler: (displayId: number) => void): Promise<UnlistenFn> =>
  listen<number>("viewer-display-changed", (event) => handler(event.payload));
export const onSessionEnded = (handler: () => void): Promise<UnlistenFn> => listen("viewer-ended", handler);
