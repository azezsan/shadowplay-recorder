<script lang="ts">
  import { commands, type Phase } from "$lib/bindings";
  import * as Card from "$lib/components/ui/card";
  import { status } from "$lib/status.svelte";

  // The wording comes with the status, from the app, so the tray says the
  // same.
  const dots: Record<Phase, string> = {
    idle: "bg-muted-foreground/40",
    gameRunning: "bg-sky-500",
    inMatch: "bg-amber-500",
    starting: "bg-amber-500 animate-pulse",
    recording: "bg-red-500 animate-pulse",
    finishing: "bg-red-500",
  };

  let version = $state<string | null>(null);
  void commands.getVersion().then((v) => (version = v));

  const dot = $derived(status.current ? dots[status.current.phase] : null);
  const short = $derived(status.current?.storage?.short ? status.current.storage : null);
</script>

<Card.Root>
  <Card.Content class="flex items-center gap-3">
    <span class={["size-3 shrink-0 rounded-full", dot ?? "bg-muted"]}></span>
    <p class="min-w-0 flex-1 font-medium" aria-live="polite">
      {status.current?.label ?? "Starting"}
    </p>
    {#if version}
      <span class="text-muted-foreground shrink-0 text-xs">v{version}</span>
    {/if}
  </Card.Content>
  {#if short}
    <Card.Content>
      <p class="text-destructive text-sm" role="alert">
        {short.disk} has {short.freeGb} GB free, under its {short.reserveGb} GB reserve, and deleting
        every recording would not make room. The next recording may not fit: free up space on the disk.
      </p>
    </Card.Content>
  {/if}
</Card.Root>
