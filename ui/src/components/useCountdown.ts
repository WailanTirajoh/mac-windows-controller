import { useEffect, useState } from "react";

/** Sisa detik sampai `deadline` (milidetik epoch), diperbarui tiap detik. */
export function useCountdown(deadline: number): number {
  const [now, setNow] = useState(Date.now);

  useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [deadline]);

  return Math.max(0, Math.ceil((deadline - now) / 1000));
}

export const formatSeconds = (seconds: number): string =>
  `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
