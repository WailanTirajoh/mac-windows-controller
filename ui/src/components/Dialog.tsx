import type { ReactNode } from "react";

/** Dialog modal di tengah jendela. */
export function Dialog({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-black/40 p-6">
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className="w-full max-w-sm rounded-xl bg-white p-6 shadow-xl dark:bg-neutral-800"
      >
        <h2 className="text-base font-semibold">{title}</h2>
        {children}
      </div>
    </div>
  );
}

const BUTTON = "rounded-lg px-3.5 py-2 text-sm font-medium transition-colors disabled:opacity-50";

export const PRIMARY_BUTTON = `${BUTTON} bg-teal-600 text-white hover:bg-teal-700`;
export const SECONDARY_BUTTON = `${BUTTON} bg-neutral-100 text-neutral-800 hover:bg-neutral-200 dark:bg-neutral-700 dark:text-neutral-100 dark:hover:bg-neutral-600`;
