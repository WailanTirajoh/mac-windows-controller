import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export type Os = "macos" | "windows" | "linux" | "unknown";

export interface SelfInfo {
  fingerprint: string;
  id: string;
  name: string;
  os: Os;
  port: number;
  acceptIncoming: boolean;
}

export interface Device {
  fingerprint: string;
  id: string;
  name: string;
  os: Os;
  /** Terlihat di jaringan saat ini. */
  online: boolean;
  /** Sudah tersimpan sebagai perangkat terpercaya. */
  paired: boolean;
  /** Boleh mengendalikan perangkat ini. */
  allowIncoming: boolean;
}

export type ConnectOutcome =
  | { status: "connected"; device: Device }
  | { status: "needs_code"; device: Device; expiresInSecs: number };

/** Perangkat lain meminta pairing; `code` harus ditampilkan. */
export interface PairingRequest {
  fingerprint: string;
  name: string;
  os: Os;
  code: string;
  expiresInSecs: number;
}

export interface PairingFinished {
  fingerprint: string;
  paired: boolean;
}

/** Perangkat yang sedang melihat atau mengendalikan layar ini. */
export interface Controller {
  fingerprint: string;
  name: string;
  inputEnabled: boolean;
}

/** Izin OS yang dibutuhkan supaya perangkat ini bisa dikendalikan (hanya relevan di macOS). */
export interface Permissions {
  screenRecording: boolean;
  accessibility: boolean;
}

export type PermissionKind = "screen_recording" | "accessibility";

export interface Transfer {
  id: number;
  name: string;
  size: number;
  transferred: number;
  /** `true` untuk file yang diterima. */
  incoming: boolean;
  peerName: string;
  status: "active" | "done" | "failed";
  /** Lokasi file setelah selesai. */
  path: string | null;
  error: string | null;
}

export const getSelf = () => invoke<SelfInfo>("get_self");
export const listDevices = () => invoke<Device[]>("list_devices");
export const connectDevice = (fingerprint: string) => invoke<ConnectOutcome>("connect_device", { fingerprint });
export const connectAddress = (address: string) => invoke<ConnectOutcome>("connect_address", { address });
export const submitPairingCode = (fingerprint: string, code: string, allowReverse: boolean) =>
  invoke<void>("submit_pairing_code", { fingerprint, code, allowReverse });
export const disconnectDevice = (fingerprint: string) => invoke<void>("disconnect_device", { fingerprint });
export const forgetDevice = (fingerprint: string) => invoke<void>("forget_device", { fingerprint });
export const rejectPairing = (fingerprint: string) => invoke<void>("reject_pairing", { fingerprint });
export const setAcceptIncoming = (accept: boolean) => invoke<void>("set_accept_incoming", { accept });
export const openViewer = (fingerprint: string) => invoke<void>("open_viewer", { fingerprint });
export const listControllers = () => invoke<Controller[]>("list_controllers");
export const endIncomingSession = (fingerprint: string) => invoke<void>("end_incoming_session", { fingerprint });
export const getPermissions = () => invoke<Permissions>("get_permissions");
export const requestPermission = (kind: PermissionKind) => invoke<void>("request_permission", { kind });
export const listTransfers = () => invoke<Transfer[]>("list_transfers");
export const sendFiles = (fingerprint: string, paths: string[]) => invoke<void>("send_files", { fingerprint, paths });
export const cancelTransfer = (id: number) => invoke<void>("cancel_transfer", { id });
export const clearTransfers = () => invoke<void>("clear_transfers");
export const revealTransfer = (id: number) => invoke<void>("reveal_transfer", { id });

/** Membuka dialog pilih file lalu mengirim pilihannya ke `fingerprint`. */
export async function pickAndSendFiles(fingerprint: string): Promise<void> {
  const picked = await open({ multiple: true, title: "Pilih file untuk dikirim" });
  if (picked === null) return;
  await sendFiles(fingerprint, Array.isArray(picked) ? picked : [picked]);
}

export const onDevicesChanged = (handler: () => void): Promise<UnlistenFn> => listen("devices-changed", handler);
export const onControllersChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("controllers-changed", handler);
export const onTransfersChanged = (handler: () => void): Promise<UnlistenFn> => listen("transfers-changed", handler);
export const onPairingRequested = (handler: (request: PairingRequest) => void): Promise<UnlistenFn> =>
  listen<PairingRequest>("pairing-requested", (event) => handler(event.payload));
export const onPairingFinished = (handler: (result: PairingFinished) => void): Promise<UnlistenFn> =>
  listen<PairingFinished>("pairing-finished", (event) => handler(event.payload));

/** Pesan error dari perintah Rust datang sebagai string, sebagian berawalan huruf kecil. */
export const errorMessage = (error: unknown): string => {
  const text = typeof error === "string" ? error : String(error);
  return text.charAt(0).toUpperCase() + text.slice(1);
};
