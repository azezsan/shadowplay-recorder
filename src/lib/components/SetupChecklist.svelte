<!--
  The setup checklist: the overlay, the console log and the recordings
  folder. It re-checks while the window is visible and shrinks to one line
  once everything is ready.
-->
<script lang="ts">
  import CircleCheck from "@lucide/svelte/icons/circle-check";
  import type { ConsoleLogState, FolderState, OverlayState } from "$lib/bindings";
  import SetupCheck from "$lib/components/SetupCheck.svelte";
  import * as Card from "$lib/components/ui/card";
  import { setup, watchSetup } from "$lib/setup.svelte";

  type Explained = { title: string; help?: string; steps?: string[] };

  const overlay: Record<OverlayState, Explained> = {
    ready: { title: "The NVIDIA overlay is ready" },
    nvidiaAppMissing: {
      title: "The NVIDIA App is not installed",
      help: "Install the NVIDIA App from nvidia.com and turn on its overlay. ShadowPlay records through it.",
    },
    switchMissing: {
      title: "The overlay's debug switch is missing",
      help: "The overlay's config needs one line that lets this app press Record. An NVIDIA App update can remove it.",
    },
    notReachable: {
      title: "The NVIDIA App needs a full restart",
      help: "Right-click the NVIDIA icon in the Windows tray and quit the NVIDIA App, then open it again. Closing its window is not enough.",
    },
    noRecordPage: {
      title: "The overlay has not loaded yet",
      help: "Wait a few seconds. If this stays, open the overlay once with Alt+Z.",
    },
  };

  const consoleLog: Record<ConsoleLogState, Explained> = {
    ready: { title: "Deadlock writes its console log" },
    gameNotFound: {
      title: "Deadlock was not found in Steam",
      help: "Install Deadlock through Steam. If it is installed already, set the console log's location in Settings.",
    },
    optionMissing: {
      title: "Deadlock is not writing its console log",
      help: "Add the -condebug launch option in Steam:",
      steps: [
        "In Steam, open your Library.",
        "Right-click Deadlock and choose Properties.",
        "On the General tab, find Launch Options.",
        "Type -condebug in the box, after a space if it already holds other options.",
        "Close the window. If Deadlock is running, restart it.",
      ],
    },
  };

  const recordingsFolder: Record<FolderState, Explained> = {
    ready: { title: "The recordings folder is set" },
    notSet: {
      title: "No recordings folder is set",
      help: "Choose the folder ShadowPlay saves recordings to. Nothing is recorded until one is set.",
    },
    missing: {
      title: "The recordings folder does not exist",
      help: "Create the folder again, or choose the one ShadowPlay saves recordings to now.",
    },
  };

  const report = $derived(setup.report);
  const allReady = $derived(
    report !== null &&
      report.overlay === "ready" &&
      report.consoleLog === "ready" &&
      report.recordingsFolder === "ready",
  );

  $effect(() => watchSetup());
</script>

<Card.Root>
  {#if report === null}
    <Card.Content>
      <p class="text-muted-foreground">Checking the setup…</p>
    </Card.Content>
  {:else if allReady}
    <Card.Content class="flex flex-row items-center gap-3">
      <CircleCheck class="size-5 shrink-0 text-emerald-600" aria-hidden="true" />
      <p class="font-medium">Ready to record</p>
    </Card.Content>
  {:else}
    <Card.Header>
      <Card.Title>Setup</Card.Title>
      <Card.Description>Matches are recorded once all three are ready.</Card.Description>
    </Card.Header>
    <Card.Content>
      <ul class="flex flex-col gap-4" aria-live="polite">
        <!-- Each check takes its fix button as an `action` snippet. -->
        <SetupCheck ready={report.overlay === "ready"} {...overlay[report.overlay]} />
        <SetupCheck ready={report.consoleLog === "ready"} {...consoleLog[report.consoleLog]} />
        <SetupCheck
          ready={report.recordingsFolder === "ready"}
          {...recordingsFolder[report.recordingsFolder]}
        />
      </ul>
    </Card.Content>
  {/if}
</Card.Root>
