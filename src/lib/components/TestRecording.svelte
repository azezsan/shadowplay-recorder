<!--
  The setup checklist's "Test recording" button: records a few seconds to
  prove the overlay, the record command and the recordings folder work
  together, then says what happened. It is disabled during a match and while
  no recordings folder is set; the app refuses the test then as well.
-->
<script lang="ts">
  import LoaderCircle from "@lucide/svelte/icons/loader-circle";
  import { commands, type TestRecordingError } from "$lib/bindings";
  import { Button } from "$lib/components/ui/button";
  import { status } from "$lib/status.svelte";

  let { folderSet }: { folderSet: boolean } = $props();

  let running = $state(false);
  let result = $state<{ text: string; error: boolean } | null>(null);

  const phase = $derived(status.current?.phase ?? null);
  const outOfMatch = $derived(phase === "idle" || phase === "gameRunning");
  const disabledReason = $derived(
    !folderSet
      ? "Set a recordings folder first."
      : !outOfMatch
        ? "Available again once the match is over."
        : null,
  );

  function fileName(path: string): string {
    return path.split(/[\\/]/).pop() ?? path;
  }

  function explain(error: TestRecordingError): string {
    switch (error.kind) {
      case "busy":
        return "A match is in progress, so the test was not run. Try again once it is over.";
      case "noRecordingsFolder":
        return "No recordings folder is set. Choose one in Settings, then test again.";
      case "overlayUnreachable":
        return "The NVIDIA overlay could not be reached, so nothing was recorded. Fix the overlay above, then test again.";
      case "startRejected":
        return `The NVIDIA overlay refused to start recording (${error.detail}). Check that ShadowPlay's Record works with Alt+F9, then test again.`;
      case "noRecordingAppeared":
        return `The record commands were sent, but no recording appeared in ${error.folder}. ShadowPlay probably saves somewhere else: check its folder in the overlay's settings (Alt+Z, Settings, Recordings) and set the same one here.`;
      case "stopFailed":
        return `Recording started (${fileName(error.recording)}) but could not be stopped (${error.detail}). Press Alt+F9 to stop it.`;
    }
  }

  async function test() {
    running = true;
    result = null;
    try {
      const recording = await commands.testRecording();
      result = { text: `Recorded ${fileName(recording)}. Everything works.`, error: false };
    } catch (e) {
      const error = e as TestRecordingError;
      result = { text: typeof error === "object" ? explain(error) : String(e), error: true };
    } finally {
      running = false;
    }
  }
</script>

<div class="flex flex-col gap-2">
  <div class="flex flex-row flex-wrap items-center gap-3">
    <Button
      variant="outline"
      size="sm"
      onclick={test}
      disabled={running || disabledReason !== null}
    >
      {#if running}
        <LoaderCircle class="animate-spin" aria-hidden="true" />
        Recording a few seconds…
      {:else}
        Test recording
      {/if}
    </Button>
    {#if disabledReason && !running}
      <p class="text-muted-foreground text-sm">{disabledReason}</p>
    {/if}
  </div>
  {#if result}
    <p
      class={["text-sm", result.error ? "text-destructive" : "text-emerald-700"]}
      role={result.error ? "alert" : "status"}
    >
      {result.text}
    </p>
  {/if}
</div>
