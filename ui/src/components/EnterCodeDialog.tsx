import { type FormEvent, useState } from "react";
import { type Device, errorMessage, submitPairingCode } from "../api";
import { Dialog, PRIMARY_BUTTON, SECONDARY_BUTTON } from "./Dialog";
import { formatSeconds, useCountdown } from "./useCountdown";

interface Props {
  device: Device;
  /** Waktu kode kedaluwarsa, milidetik epoch. */
  expiresAt: number;
  onPaired: () => void;
  onCancel: () => void;
}

/** Dialog di sisi pengendali: mengetik kode yang tampil di perangkat tujuan. */
export function EnterCodeDialog({ device, expiresAt, onPaired, onCancel }: Props) {
  const [code, setCode] = useState("");
  const [allowReverse, setAllowReverse] = useState(true);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const remaining = useCountdown(expiresAt);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await submitPairingCode(device.fingerprint, code, allowReverse);
      onPaired();
    } catch (failure) {
      setError(errorMessage(failure));
      setCode("");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={`Hubungkan ke ${device.name}`}>
      <form onSubmit={submit}>
        <p className="mt-2 text-sm text-neutral-600 dark:text-neutral-300">
          Ketik 6 angka yang sedang tampil di layar <b>{device.name}</b>.
        </p>
        <input
          autoFocus
          inputMode="numeric"
          autoComplete="one-time-code"
          aria-label="Kode pairing"
          placeholder="000000"
          maxLength={6}
          value={code}
          onChange={(event) => setCode(event.target.value.replace(/\D/g, ""))}
          className="mt-4 w-full rounded-lg border border-neutral-300 bg-transparent px-3 py-2.5 text-center font-mono text-2xl tracking-[0.4em] outline-none focus:border-teal-600 dark:border-neutral-600"
        />
        <p className="mt-2 min-h-5 text-sm text-red-600 dark:text-red-400" role="alert">
          {error}
        </p>
        <label className="mt-1 flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-0.5"
            checked={allowReverse}
            onChange={(event) => setAllowReverse(event.target.checked)}
          />
          <span>Izinkan juga {device.name} mengontrol perangkat ini</span>
        </label>
        <div className="mt-5 flex items-center justify-between">
          <span className="text-xs text-neutral-500">
            {remaining > 0 ? `Kode berlaku ${formatSeconds(remaining)} lagi` : "Kode sudah kedaluwarsa"}
          </span>
          <div className="flex gap-2">
            <button type="button" className={SECONDARY_BUTTON} onClick={onCancel}>
              Batal
            </button>
            <button type="submit" className={PRIMARY_BUTTON} disabled={busy || code.length !== 6}>
              Hubungkan
            </button>
          </div>
        </div>
      </form>
    </Dialog>
  );
}
