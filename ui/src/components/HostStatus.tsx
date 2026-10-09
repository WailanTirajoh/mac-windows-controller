import { type Controller, type PermissionKind, type Permissions, endIncomingSession, requestPermission } from "../api";
import { SECONDARY_BUTTON } from "./Dialog";

const BANNER = "mb-3 flex items-center justify-between gap-4 rounded-lg px-4 py-2.5 text-sm";

/** Siapa yang sedang mengendalikan perangkat ini, dengan tombol untuk memutusnya. */
export function ControllerBanners({ controllers }: { controllers: Controller[] }) {
  return (
    <>
      {controllers.map((controller) => (
        <div
          key={controller.fingerprint}
          role="status"
          className={`${BANNER} bg-amber-100 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100`}
        >
          <span>
            <b>{controller.name}</b> sedang {controller.inputEnabled ? "mengendalikan" : "melihat"} layar ini.
          </span>
          <button
            type="button"
            className={SECONDARY_BUTTON}
            onClick={() => void endIncomingSession(controller.fingerprint)}
          >
            Putuskan
          </button>
        </div>
      ))}
    </>
  );
}

const MISSING: { kind: PermissionKind; granted: (permissions: Permissions) => boolean; text: string }[] = [
  {
    kind: "screen_recording",
    granted: (permissions) => permissions.screenRecording,
    text: "Supaya perangkat lain bisa melihat layar ini, izinkan Sambung di Screen Recording.",
  },
  {
    kind: "accessibility",
    granted: (permissions) => permissions.accessibility,
    text: "Supaya perangkat lain bisa menggerakkan mouse dan keyboard di sini, izinkan Sambung di Accessibility.",
  },
];

/** Izin macOS yang belum diberikan. Tidak menampilkan apa-apa kalau semua sudah ada. */
export function PermissionBanners({ permissions, onRecheck }: { permissions: Permissions; onRecheck: () => void }) {
  return (
    <>
      {MISSING.filter((item) => !item.granted(permissions)).map((item) => (
        <div
          key={item.kind}
          className={`${BANNER} bg-neutral-200 text-neutral-800 dark:bg-neutral-800 dark:text-neutral-200`}
        >
          <span>{item.text}</span>
          <span className="flex shrink-0 gap-2">
            <button type="button" className={SECONDARY_BUTTON} onClick={() => void requestPermission(item.kind)}>
              Buka Pengaturan
            </button>
            <button type="button" className={SECONDARY_BUTTON} onClick={onRecheck}>
              Cek lagi
            </button>
          </span>
        </div>
      ))}
    </>
  );
}
