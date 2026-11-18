<script lang="ts">
  import { onMount } from "svelte";
  import {
    ACTIVITY_KEPT_IN_MEMORY as KEPT,
    commands,
    events,
    type ActivityEntry,
    type Level,
  } from "$lib/bindings";
  import * as Card from "$lib/components/ui/card";

  const levels: Record<Level, { label: string; row: string; tag: string }> = {
    info: { label: "Info", row: "", tag: "text-muted-foreground" },
    warning: {
      label: "Warning",
      row: "bg-amber-500/10",
      tag: "text-amber-700 dark:text-amber-400 font-medium",
    },
    error: {
      label: "Error",
      row: "bg-destructive/10",
      tag: "text-destructive font-medium",
    },
  };

  let entries = $state<ActivityEntry[]>([]);
  let list = $state<HTMLElement>();
  let followNewest = true;

  const key = (e: ActivityEntry) => `${e.time} ${e.level} ${e.message}`;

  function keep(next: ActivityEntry[]) {
    entries = next.length > KEPT ? next.slice(-KEPT) : next;
  }

  onMount(() => {
    let loaded = false;
    const early: ActivityEntry[] = [];
    const unlisten = events.activityAdded.listen((event) => {
      if (loaded) keep([...entries, event.payload]);
      else early.push(event.payload);
    });
    void commands.getRecentActivity().then((recent) => {
      // Entries that arrived while loading may already be in `recent`.
      const seen = new Set(recent.map(key));
      keep([...recent, ...early.filter((e) => !seen.has(key(e)))]);
      loaded = true;
    });
    return () => void unlisten.then((stop) => stop());
  });

  function onscroll() {
    if (!list) return;
    followNewest = list.scrollHeight - list.scrollTop - list.clientHeight < 8;
  }

  $effect(() => {
    void entries.length;
    if (list && followNewest) list.scrollTop = list.scrollHeight;
  });

  function formatTime(ms: number | null): string {
    if (ms === null) return "";
    const date = new Date(ms);
    const time = date.toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
    if (date.toDateString() === new Date().toDateString()) return time;
    return `${date.toLocaleDateString([], { month: "short", day: "numeric" })} ${time}`;
  }
</script>

<Card.Root>
  <Card.Header>
    <Card.Title>Activity</Card.Title>
  </Card.Header>
  <Card.Content>
    {#if entries.length === 0}
      <p class="text-muted-foreground text-sm">Nothing yet.</p>
    {:else}
      <ol
        bind:this={list}
        {onscroll}
        class="max-h-64 overflow-y-auto rounded-md border font-mono text-xs"
        aria-label="Recent activity"
      >
        {#each entries as entry, i (i)}
          <li class={["flex gap-2 px-2 py-1", levels[entry.level].row]}>
            <time class="text-muted-foreground shrink-0 tabular-nums">
              {formatTime(entry.time)}
            </time>
            <span class={["w-14 shrink-0", levels[entry.level].tag]}>
              {levels[entry.level].label}
            </span>
            <span class="min-w-0 break-words">{entry.message}</span>
          </li>
        {/each}
      </ol>
    {/if}
  </Card.Content>
</Card.Root>
