<script lang="ts">
  import { commands, type Settings } from "$lib/bindings";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Switch } from "$lib/components/ui/switch";

  // The settings as last loaded or saved, and the form's edits of them.
  let saved = $state<Settings | null>(null);
  let folder = $state<string | null>(null);
  let maxSizeGb = $state<number | null>(null);
  let stopDelaySeconds = $state<number | null>(null);
  let focusOnMatchStart = $state(true);

  let saving = $state(false);
  let message = $state<{ text: string; error: boolean } | null>(null);

  function show(settings: Settings) {
    saved = settings;
    folder = settings.recordingsFolder ?? null;
    maxSizeGb = settings.maxSizeGb ?? null;
    stopDelaySeconds = settings.stopDelaySeconds ?? null;
    focusOnMatchStart = settings.focusOnMatchStart ?? true;
  }

  const dirty = $derived(
    saved !== null &&
      (folder !== (saved.recordingsFolder ?? null) ||
        maxSizeGb !== (saved.maxSizeGb ?? null) ||
        stopDelaySeconds !== (saved.stopDelaySeconds ?? null) ||
        focusOnMatchStart !== (saved.focusOnMatchStart ?? true)),
  );

  $effect(() => {
    commands.getSettings().then(show, (e: unknown) => {
      message = { text: `Could not load the settings: ${String(e)}`, error: true };
    });
  });

  async function chooseFolder() {
    const picked = await commands.pickRecordingsFolder(folder);
    if (picked !== null) {
      folder = picked;
      message = null;
    }
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!saved) return;
    saving = true;
    try {
      // The app clamps out-of-range values; it only needs whole, non-negative
      // numbers to read them.
      show(
        await commands.saveSettings({
          ...saved,
          recordingsFolder: folder,
          maxSizeGb: Math.max(0, maxSizeGb ?? 0),
          stopDelaySeconds: Math.max(0, Math.round(stopDelaySeconds ?? 0)),
          focusOnMatchStart,
        }),
      );
      message = { text: "Saved.", error: false };
    } catch (e) {
      message = { text: String(e), error: true };
    } finally {
      saving = false;
    }
  }
</script>

<Card.Root>
  <Card.Header>
    <Card.Title>Settings</Card.Title>
  </Card.Header>
  <Card.Content>
    {#if saved}
      <form class="flex flex-col gap-5" onsubmit={save} oninput={() => (message = null)}>
        <div class="flex flex-col gap-2">
          <Label for="choose-folder">Recordings folder</Label>
          <div class="flex items-center gap-2">
            <p
              id="recordings-folder"
              class={[
                "min-w-0 flex-1 truncate text-sm",
                folder ? "font-mono" : "text-muted-foreground",
              ]}
              title={folder ?? undefined}
            >
              {folder ?? "Not set. Nothing is recorded until one is chosen."}
            </p>
            <Button
              id="choose-folder"
              type="button"
              variant="outline"
              aria-describedby="recordings-folder"
              onclick={chooseFolder}>Choose…</Button
            >
          </div>
        </div>

        <div class="grid grid-cols-2 gap-4">
          <div class="flex flex-col gap-2">
            <Label for="size-cap">Size cap (GB)</Label>
            <Input id="size-cap" type="number" min="1" step="any" bind:value={maxSizeGb} />
          </div>
          <div class="flex flex-col gap-2">
            <Label for="stop-delay">Stop delay (seconds)</Label>
            <Input
              id="stop-delay"
              type="number"
              min="0"
              max="600"
              step="1"
              bind:value={stopDelaySeconds}
            />
          </div>
        </div>

        <div class="flex items-center justify-between gap-4">
          <Label for="focus-on-start">Bring Deadlock to the front when a match is found</Label>
          <Switch
            id="focus-on-start"
            bind:checked={focusOnMatchStart}
            onCheckedChange={() => (message = null)}
          />
        </div>

        <div class="flex items-center justify-end gap-3">
          {#if message}
            <p
              class={["text-sm", message.error ? "text-destructive" : "text-muted-foreground"]}
              role={message.error ? "alert" : "status"}
            >
              {message.text}
            </p>
          {/if}
          <Button type="submit" disabled={!dirty || saving}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    {:else if message}
      <p class="text-destructive text-sm" role="alert">{message.text}</p>
    {:else}
      <p class="text-muted-foreground text-sm">Loading…</p>
    {/if}
  </Card.Content>
</Card.Root>
