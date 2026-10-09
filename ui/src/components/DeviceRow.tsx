import type { Device, Os } from "../api";
import { PRIMARY_BUTTON, SECONDARY_BUTTON } from "./Dialog";

const OS_LABEL: Record<Os, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "Linux",
  unknown: "Perangkat",
};

function DeviceIcon() {
  return (
    <svg viewBox="0 0 24 24" className="size-6" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="3" y="4.5" width="18" height="12" rx="1.8" />
      <path d="M8.5 20h7M12 16.5V20" strokeLinecap="round" />
    </svg>
  );
}

interface Props {
  device: Device;
  busy: boolean;
  onConnect: () => void;
  onSendFile: () => void;
  onForget: () => void;
}

export function DeviceRow({ device, busy, onConnect, onSendFile, onForget }: Props) {
  const status = [device.paired ? "Tersimpan" : "Belum terhubung", device.online ? "Online" : "Offline"].join(" · ");

  return (
    <li className="flex items-center gap-4 rounded-xl border border-neutral-200 bg-white px-4 py-3 dark:border-neutral-700 dark:bg-neutral-800">
      <div
        className={`flex size-10 shrink-0 items-center justify-center rounded-lg ${
          device.online
            ? "bg-teal-50 text-teal-700 dark:bg-teal-950 dark:text-teal-300"
            : "bg-neutral-100 text-neutral-400 dark:bg-neutral-700"
        }`}
      >
        <DeviceIcon />
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium">{device.name}</p>
        <p className="truncate text-sm text-neutral-500 dark:text-neutral-400">
          {OS_LABEL[device.os]} · {status}
        </p>
      </div>
      {device.paired && (
        <button
          type="button"
          className="rounded-lg px-2.5 py-2 text-sm text-neutral-500 hover:bg-neutral-100 hover:text-red-600 dark:hover:bg-neutral-700"
          onClick={onForget}
        >
          Lupakan
        </button>
      )}
      {device.paired && (
        <button type="button" className={SECONDARY_BUTTON} onClick={onSendFile}>
          Kirim file
        </button>
      )}
      <button type="button" className={PRIMARY_BUTTON} disabled={busy} onClick={onConnect}>
        {busy ? "Menghubungkan…" : "Hubungkan"}
      </button>
    </li>
  );
}
