<script lang="ts">
  import type { Phase } from "$lib/bindings";
  import * as Card from "$lib/components/ui/card";
  import { status } from "$lib/status.svelte";

  const phases: Record<Phase, { label: string; dot: string }> = {
    idle: { label: "Waiting for Deadlock", dot: "bg-muted-foreground/40" },
    gameRunning: { label: "Deadlock is running", dot: "bg-sky-500" },
    inMatch: { label: "In a match, not recording", dot: "bg-amber-500" },
    starting: { label: "Starting the recording", dot: "bg-amber-500 animate-pulse" },
    recording: { label: "Recording", dot: "bg-red-500 animate-pulse" },
    finishing: { label: "Saving the recording", dot: "bg-red-500" },
  };

  const phase = $derived(status.current ? phases[status.current.phase] : null);
</script>

<Card.Root>
  <Card.Content class="flex items-center gap-3">
    <span class={["size-3 shrink-0 rounded-full", phase?.dot ?? "bg-muted"]}></span>
    <div class="min-w-0">
      <p class="font-medium" aria-live="polite">{phase?.label ?? "Starting"}</p>
      {#if status.current?.matchId}
        <p class="text-muted-foreground text-sm">Match {status.current.matchId}</p>
      {/if}
    </div>
  </Card.Content>
</Card.Root>
