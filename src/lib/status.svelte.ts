// The recorder's live status, shared by every component that shows it.

import { commands, events, type Status } from "$lib/bindings";

export const status = $state<{ current: Status | null }>({ current: null });

/** Loads the status once and keeps it up to date. Call once at startup. */
export async function followStatus(): Promise<void> {
  await events.statusChanged.listen((event) => {
    status.current = event.payload;
  });
  status.current = await commands.getStatus();
}
