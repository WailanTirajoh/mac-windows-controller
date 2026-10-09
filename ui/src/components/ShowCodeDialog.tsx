import type { PairingRequest } from "../api";
import { Dialog, SECONDARY_BUTTON } from "./Dialog";
import { formatSeconds, useCountdown } from "./useCountdown";

interface Props {
  request: PairingRequest;
  /** Waktu kode kedaluwarsa, milidetik epoch. */
  expiresAt: number;
  onReject: () => void;
}

/** Dialog di sisi tujuan: menampilkan kode untuk diketik di perangkat peminta. */
export function ShowCodeDialog({ request, expiresAt, onReject }: Props) {
  const remaining = useCountdown(expiresAt);

  return (
    <Dialog title={`${request.name} ingin terhubung`}>
      <p className="mt-2 text-sm text-neutral-600 dark:text-neutral-300">
        Ketik kode ini di <b>{request.name}</b>. Setelah terhubung, perangkat itu bisa melihat dan mengendalikan layar
        ini.
      </p>
      <p
        aria-label="Kode pairing"
        className="mt-4 rounded-lg bg-neutral-100 py-4 text-center font-mono text-4xl font-semibold tracking-[0.3em] select-text dark:bg-neutral-900"
      >
        {request.code.slice(0, 3)} {request.code.slice(3)}
      </p>
      <div className="mt-5 flex items-center justify-between">
        <span className="text-xs text-neutral-500">Berlaku {formatSeconds(remaining)} lagi</span>
        <button type="button" className={SECONDARY_BUTTON} onClick={onReject}>
          Tolak
        </button>
      </div>
    </Dialog>
  );
}
