import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  type ConnectOutcome,
  type Controller,
  type Device,
  type PairingRequest,
  type Permissions,
  type SelfInfo,
  connectAddress,
  connectDevice,
  disconnectDevice,
  errorMessage,
  forgetDevice,
  getPermissions,
  getSelf,
  listControllers,
  listDevices,
  onControllersChanged,
  onDevicesChanged,
  onPairingFinished,
  onPairingRequested,
  openViewer,
  rejectPairing,
  setAcceptIncoming,
} from "./api";
import { DeviceRow } from "./components/DeviceRow";
import { PRIMARY_BUTTON } from "./components/Dialog";
import { EnterCodeDialog } from "./components/EnterCodeDialog";
import { ControllerBanners, PermissionBanners } from "./components/HostStatus";
import { ShowCodeDialog } from "./components/ShowCodeDialog";

/** Kode yang harus diketik di sini, untuk perangkat yang sedang dihubungi. */
interface OutgoingPairing {
  device: Device;
  expiresAt: number;
}

/** Kode yang sedang ditampilkan di sini, untuk perangkat yang meminta. */
interface IncomingPairing {
  request: PairingRequest;
  expiresAt: number;
}

interface Notice {
  kind: "info" | "error";
  text: string;
}

export function App() {
  const [self, setSelf] = useState<SelfInfo | null>(null);
  const [devices, setDevices] = useState<Device[]>([]);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [outgoing, setOutgoing] = useState<OutgoingPairing | null>(null);
  const [incoming, setIncoming] = useState<IncomingPairing[]>([]);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [address, setAddress] = useState("");
  const [controllers, setControllers] = useState<Controller[]>([]);
  const [permissions, setPermissions] = useState<Permissions | null>(null);

  const refresh = useCallback(() => {
    listDevices().then(setDevices, (error) => setNotice({ kind: "error", text: errorMessage(error) }));
  }, []);
  const refreshControllers = useCallback(() => void listControllers().then(setControllers), []);
  const refreshPermissions = useCallback(() => void getPermissions().then(setPermissions), []);

  useEffect(() => {
    getSelf().then(setSelf, (error) => setNotice({ kind: "error", text: errorMessage(error) }));
    refresh();
    refreshControllers();
    refreshPermissions();
    // Izin diubah di System Settings, jadi dicek lagi saat jendela ini kembali aktif.
    window.addEventListener("focus", refreshPermissions);

    const subscriptions = [
      onDevicesChanged(refresh),
      onControllersChanged(refreshControllers),
      onPairingRequested((request) =>
        setIncoming((current) => [
          ...current.filter((item) => item.request.fingerprint !== request.fingerprint),
          { request, expiresAt: Date.now() + request.expiresInSecs * 1000 },
        ]),
      ),
      onPairingFinished(({ fingerprint }) =>
        setIncoming((current) => current.filter((item) => item.request.fingerprint !== fingerprint)),
      ),
    ];
    return () => {
      window.removeEventListener("focus", refreshPermissions);
      for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten());
    };
  }, [refresh, refreshControllers, refreshPermissions]);

  /** Membuka jendela yang menampilkan layar `device`. */
  const view = async (device: Device) => {
    try {
      await openViewer(device.fingerprint);
    } catch (error) {
      setNotice({ kind: "error", text: errorMessage(error) });
    }
  };

  const connect = async (key: string, start: () => Promise<ConnectOutcome>) => {
    setConnecting(key);
    setNotice(null);
    try {
      const outcome = await start();
      if (outcome.status === "needs_code") {
        setOutgoing({ device: outcome.device, expiresAt: Date.now() + outcome.expiresInSecs * 1000 });
      } else {
        await view(outcome.device);
      }
    } catch (error) {
      setNotice({ kind: "error", text: errorMessage(error) });
    } finally {
      setConnecting(null);
    }
  };

  const connectByAddress = (event: FormEvent) => {
    event.preventDefault();
    if (address.trim() !== "") void connect("address", () => connectAddress(address));
  };

  const forget = async (device: Device) => {
    try {
      await forgetDevice(device.fingerprint);
      setNotice({ kind: "info", text: `${device.name} dilupakan. Untuk terhubung lagi perlu kode baru.` });
    } catch (error) {
      setNotice({ kind: "error", text: errorMessage(error) });
    }
  };

  const toggleAccept = async (accept: boolean) => {
    await setAcceptIncoming(accept);
    setSelf((current) => current && { ...current, acceptIncoming: accept });
  };

  const shownCode = incoming[0];

  return (
    <div className="flex h-full flex-col bg-neutral-50 text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <header className="flex items-center justify-between border-b border-neutral-200 px-6 py-4 dark:border-neutral-800">
        <div>
          <h1 className="text-lg font-semibold">Sambung</h1>
          <p className="text-sm text-neutral-500 dark:text-neutral-400">
            {self ? `${self.name} · ID ${self.id}` : "Memulai…"}
          </p>
        </div>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={self?.acceptIncoming ?? true}
            disabled={self === null}
            onChange={(event) => void toggleAccept(event.target.checked)}
          />
          Terima koneksi masuk
        </label>
      </header>

      <main className="flex-1 overflow-y-auto px-6 py-5">
        <ControllerBanners controllers={controllers} />
        {permissions && <PermissionBanners permissions={permissions} onRecheck={refreshPermissions} />}
        {notice && (
          <p
            role={notice.kind === "error" ? "alert" : "status"}
            className={`mb-4 rounded-lg px-4 py-2.5 text-sm ${
              notice.kind === "error"
                ? "bg-red-50 text-red-700 dark:bg-red-950 dark:text-red-300"
                : "bg-teal-50 text-teal-800 dark:bg-teal-950 dark:text-teal-200"
            }`}
          >
            {notice.text}
          </p>
        )}

        <h2 className="mb-3 text-sm font-medium text-neutral-500 dark:text-neutral-400">Perangkat</h2>
        {devices.length === 0 ? (
          <p className="rounded-xl border border-dashed border-neutral-300 px-6 py-10 text-center text-sm text-neutral-500 dark:border-neutral-700">
            Belum ada perangkat lain yang terlihat.
            <br />
            Buka Sambung di perangkat lain yang terhubung ke WiFi yang sama.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {devices.map((device) => (
              <DeviceRow
                key={device.fingerprint}
                device={device}
                busy={connecting === device.fingerprint}
                onConnect={() => void connect(device.fingerprint, () => connectDevice(device.fingerprint))}
                onForget={() => void forget(device)}
              />
            ))}
          </ul>
        )}

        <form onSubmit={connectByAddress} className="mt-6">
          <label htmlFor="address" className="mb-2 block text-sm font-medium text-neutral-500 dark:text-neutral-400">
            Perangkat tidak muncul? Hubungkan lewat alamat IP
          </label>
          <div className="flex gap-2">
            <input
              id="address"
              value={address}
              onChange={(event) => setAddress(event.target.value)}
              placeholder="192.168.1.20"
              spellCheck={false}
              className="min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 py-2 text-sm outline-none focus:border-teal-600 dark:border-neutral-700 dark:bg-neutral-800"
            />
            <button type="submit" className={PRIMARY_BUTTON} disabled={connecting !== null || address.trim() === ""}>
              {connecting === "address" ? "Menghubungkan…" : "Hubungkan"}
            </button>
          </div>
        </form>
      </main>

      {outgoing && (
        <EnterCodeDialog
          device={outgoing.device}
          expiresAt={outgoing.expiresAt}
          onPaired={() => {
            setOutgoing(null);
            void view(outgoing.device);
          }}
          onCancel={() => {
            void disconnectDevice(outgoing.device.fingerprint);
            setOutgoing(null);
          }}
        />
      )}
      {shownCode && (
        <ShowCodeDialog
          request={shownCode.request}
          expiresAt={shownCode.expiresAt}
          onReject={() => void rejectPairing(shownCode.request.fingerprint)}
        />
      )}
    </div>
  );
}
