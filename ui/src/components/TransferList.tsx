import { type Transfer, cancelTransfer, clearTransfers, revealTransfer } from "../api";

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 || value >= 100 ? 0 : 1)} ${UNITS[unit]}`;
}

const LINK = "shrink-0 rounded-md px-2 py-1 text-xs text-teal-700 hover:bg-teal-50 dark:text-teal-300 dark:hover:bg-teal-950";

function TransferRow({ transfer }: { transfer: Transfer }) {
  const percent = transfer.size === 0 ? 100 : Math.round((transfer.transferred / transfer.size) * 100);
  const direction = transfer.incoming ? `dari ${transfer.peerName}` : `ke ${transfer.peerName}`;

  return (
    <li className="rounded-xl border border-neutral-200 bg-white px-4 py-3 dark:border-neutral-700 dark:bg-neutral-800">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{transfer.name}</p>
          <p className="truncate text-xs text-neutral-500 dark:text-neutral-400">
            {transfer.status === "active" &&
              `${formatBytes(transfer.transferred)} dari ${formatBytes(transfer.size)} · ${direction}`}
            {transfer.status === "done" &&
              `${formatBytes(transfer.size)} · ${transfer.incoming ? "diterima" : "terkirim"} ${direction}`}
            {transfer.status === "failed" && (
              <span className="text-red-600 dark:text-red-400">
                Gagal {direction}: {transfer.error}
              </span>
            )}
          </p>
        </div>
        {transfer.status === "active" && (
          <button type="button" className={LINK} onClick={() => void cancelTransfer(transfer.id)}>
            Batal
          </button>
        )}
        {transfer.status === "done" && transfer.incoming && (
          <button type="button" className={LINK} onClick={() => void revealTransfer(transfer.id)}>
            Tampilkan
          </button>
        )}
      </div>
      {transfer.status === "active" && (
        <div
          role="progressbar"
          aria-valuenow={percent}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label={`Kemajuan ${transfer.name}`}
          className="mt-2 h-1.5 overflow-hidden rounded-full bg-neutral-200 dark:bg-neutral-700"
        >
          <div className="h-full rounded-full bg-teal-600" style={{ width: `${percent}%` }} />
        </div>
      )}
    </li>
  );
}

/** Transfer file yang sedang berjalan dan yang sudah selesai, terbaru di atas. */
export function TransferList({ transfers }: { transfers: Transfer[] }) {
  if (transfers.length === 0) return null;
  const hasFinished = transfers.some((transfer) => transfer.status !== "active");

  return (
    <section className="mt-6">
      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-sm font-medium text-neutral-500 dark:text-neutral-400">Transfer file</h2>
        {hasFinished && (
          <button type="button" className={LINK} onClick={() => void clearTransfers()}>
            Bersihkan yang selesai
          </button>
        )}
      </div>
      <ul className="flex flex-col gap-2">
        {[...transfers].reverse().map((transfer) => (
          <TransferRow key={transfer.id} transfer={transfer} />
        ))}
      </ul>
    </section>
  );
}
