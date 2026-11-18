// The setup checklist, re-checked on a timer while the window is visible.

import { commands, type SetupReport } from "$lib/bindings";

/** How often the checklist is re-checked while the window is visible. */
const RECHECK_INTERVAL_MS = 3000;

export const setup = $state<{ report: SetupReport | null }>({ report: null });

let pending: Promise<void> | null = null;

/**
 * Checks the setup now. A fix calls this when it is done, so the checklist
 * shows the result without waiting for the timer. Overlapping calls share
 * one check.
 */
export function refreshSetup(): Promise<void> {
  pending ??= commands
    .getSetupReport()
    .then((report) => {
      setup.report = report;
    })
    .finally(() => {
      pending = null;
    });
  return pending;
}

/**
 * Re-checks the setup while the window is visible: at once, then on a timer,
 * and again as soon as the window is shown. Returns a function that stops it.
 */
export function watchSetup(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  // True while hidden: the timer is off until the window is shown again.
  let paused = false;

  const tick = async () => {
    timer = undefined;
    if (stopped) return;
    if (document.visibilityState !== "visible") {
      paused = true;
      return;
    }
    try {
      await refreshSetup();
    } finally {
      if (!stopped) timer = setTimeout(() => void tick(), RECHECK_INTERVAL_MS);
    }
  };
  const onVisibility = () => {
    if (paused && document.visibilityState === "visible") {
      paused = false;
      void tick();
    }
  };

  document.addEventListener("visibilitychange", onVisibility);
  void tick();
  return () => {
    stopped = true;
    clearTimeout(timer);
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
