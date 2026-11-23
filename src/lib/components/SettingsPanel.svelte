<script lang="ts">
  import { commands, type RetentionRule, type Settings } from "$lib/bindings";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import { Switch } from "$lib/components/ui/switch";
  import { status } from "$lib/status.svelte";

  // The settings as last loaded or saved, and the form's edits of them.
  let saved = $state<Settings | null>(null);
  let folder = $state<string | null>(null);
  let consoleLog = $state<string | null>(null);
  let retentionRule = $state<RetentionRule>("sizeCap");
  let maxSizeGb = $state<number | null>(null);
  let manualReserve = $state(false);
  let reserveGb = $state<number | null>(null);
  let stopDelaySeconds = $state<number | null>(null);
  let focusOnMatchStart = $state(true);

  // Held by the OS, not the settings file, and applied as soon as it is
  // flipped. `null` until the OS has answered.
  let startWithWindows = $state<boolean | null>(null);

  let saving = $state(false);
  let message = $state<{ text: string; error: boolean } | null>(null);

  function show(settings: Settings) {
    saved = settings;
    folder = settings.recordingsFolder ?? null;
    consoleLog = settings.consoleLog ?? null;
    retentionRule = settings.retentionRule ?? "sizeCap";
    maxSizeGb = settings.maxSizeGb ?? null;
    manualReserve = settings.manualReserve ?? false;
    reserveGb = settings.reserveGb ?? null;
    stopDelaySeconds = settings.stopDelaySeconds ?? null;
    focusOnMatchStart = settings.focusOnMatchStart ?? true;
  }

  const dirty = $derived(
    saved !== null &&
      (folder !== (saved.recordingsFolder ?? null) ||
        consoleLog !== (saved.consoleLog ?? null) ||
        retentionRule !== (saved.retentionRule ?? "sizeCap") ||
        maxSizeGb !== (saved.maxSizeGb ?? null) ||
        manualReserve !== (saved.manualReserve ?? false) ||
        reserveGb !== (saved.reserveGb ?? null) ||
        stopDelaySeconds !== (saved.stopDelaySeconds ?? null) ||
        focusOnMatchStart !== (saved.focusOnMatchStart ?? true)),
  );

  // Fill the disk's last check of the saved settings.
  const storage = $derived(status.current?.storage ?? null);
  const fillDiskSaved = $derived(saved?.retentionRule === "fillDisk");

  $effect(() => {
    commands.getSettings().then(show, (e: unknown) => {
      message = { text: `Could not load the settings: ${String(e)}`, error: true };
    });
  });

  $effect(() => {
    commands.getStartWithWindows().then(
      (enabled) => (startWithWindows = enabled),
      (e: unknown) => {
        message = { text: String(e), error: true };
      },
    );
  });

  async function changeStartWithWindows(enabled: boolean) {
    message = null;
    try {
      startWithWindows = await commands.setStartWithWindows(enabled);
    } catch (e) {
      message = { text: String(e), error: true };
      // Show what the OS really has, not what was asked for.
      startWithWindows = await commands.getStartWithWindows().catch(() => null);
    }
  }

  async function chooseFolder() {
    const picked = await commands.pickRecordingsFolder(folder);
    if (picked !== null) {
      folder = picked;
      message = null;
    }
  }

  async function chooseConsoleLog() {
    const picked = await commands.pickConsoleLog(consoleLog);
    if (picked !== null) {
      consoleLog = picked;
      message = null;
    }
  }

  function clearConsoleLog() {
    consoleLog = null;
    message = null;
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
          consoleLog,
          retentionRule,
          maxSizeGb: Math.max(0, maxSizeGb ?? 0),
          manualReserve,
          reserveGb: Math.max(0, reserveGb ?? 0),
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

        <div class="flex flex-col gap-2">
          <Label for="choose-console-log">Console log</Label>
          <div class="flex items-center gap-2">
            <p
              id="console-log"
              class={[
                "min-w-0 flex-1 truncate text-sm",
                consoleLog ? "font-mono" : "text-muted-foreground",
              ]}
              title={consoleLog ?? undefined}
            >
              {consoleLog ?? "Found through Steam."}
            </p>
            {#if consoleLog}
              <Button type="button" variant="ghost" onclick={clearConsoleLog}>Clear</Button>
            {/if}
            <Button
              id="choose-console-log"
              type="button"
              variant="outline"
              aria-describedby="console-log console-log-hint"
              onclick={chooseConsoleLog}>Choose…</Button
            >
          </div>
          <p id="console-log-hint" class="text-muted-foreground text-sm">
            Only needed when Deadlock is not found through Steam.
          </p>
        </div>

        <fieldset class="flex flex-col gap-3">
          <legend class="mb-2 text-sm font-medium">Old recordings</legend>
          <RadioGroup.Root
            bind:value={retentionRule}
            onValueChange={() => (message = null)}
            class="flex flex-col gap-2"
          >
            <div class="flex items-center gap-2">
              <RadioGroup.Item value="sizeCap" id="rule-size-cap" />
              <Label for="rule-size-cap">Delete the oldest past a size cap</Label>
            </div>
            <div class="flex items-center gap-2">
              <RadioGroup.Item value="fillDisk" id="rule-fill-disk" />
              <Label for="rule-fill-disk">Fill the disk</Label>
            </div>
          </RadioGroup.Root>

          {#if retentionRule === "sizeCap"}
            <div class="flex flex-col gap-2 sm:w-1/2">
              <Label for="size-cap">Size cap (GB)</Label>
              <Input id="size-cap" type="number" min="1" step="any" bind:value={maxSizeGb} />
            </div>
          {:else}
            <p class="text-muted-foreground text-sm">
              Recordings use the whole disk. The oldest are deleted to keep enough free space for
              the next one.
            </p>
            {#if fillDiskSaved && storage}
              <p class="text-sm" role="status">
                {storage.disk}
                {storage.freeGb} GB free · keeping {storage.reserveGb} GB in reserve ({storage.automatic
                  ? "automatic"
                  : "manual"})
              </p>
            {:else if fillDiskSaved && folder}
              <p class="text-muted-foreground text-sm" role="status">
                The free space on this disk can't be read, so nothing is deleted.
              </p>
            {/if}
            <RadioGroup.Root
              value={manualReserve ? "manual" : "automatic"}
              onValueChange={(value) => {
                manualReserve = value === "manual";
                message = null;
              }}
              class="flex flex-col gap-2"
              aria-label="Free-space reserve"
            >
              <div class="flex items-center gap-2">
                <RadioGroup.Item value="automatic" id="reserve-automatic" />
                <Label for="reserve-automatic">Automatic reserve</Label>
                <span class="text-muted-foreground text-sm">1.5 × the largest recording</span>
              </div>
              <div class="flex items-center gap-2">
                <RadioGroup.Item value="manual" id="reserve-manual" />
                <Label for="reserve-manual">Manual reserve</Label>
                <Input
                  id="reserve-gb"
                  type="number"
                  min="1"
                  max={storage ? Math.floor(storage.totalGb / 2) : undefined}
                  step="any"
                  class="w-24"
                  aria-label="Manual reserve (GB)"
                  disabled={!manualReserve}
                  bind:value={reserveGb}
                />
                <span class="text-muted-foreground text-sm">GB</span>
              </div>
            </RadioGroup.Root>
          {/if}
        </fieldset>

        <div class="grid grid-cols-2 gap-4">
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

        <div class="flex items-center justify-between gap-4">
          <div class="flex flex-col gap-1">
            <Label for="start-with-windows">Start with Windows</Label>
            <p id="start-with-windows-hint" class="text-muted-foreground text-sm">
              Opens in the tray at login, with no window. Applies right away.
            </p>
          </div>
          <Switch
            id="start-with-windows"
            aria-describedby="start-with-windows-hint"
            checked={startWithWindows ?? false}
            disabled={startWithWindows === null}
            onCheckedChange={changeStartWithWindows}
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
